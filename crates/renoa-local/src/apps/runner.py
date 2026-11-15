#!/usr/bin/env python3
"""Host-side supervisor inside each app's systemd unit managing bubblewrap + slirp4netns."""
import fcntl
import json
import os
import select
import selectors
import signal
import socket
import struct
import subprocess
import sys
import threading
import time

NS_GET_USERNS = 0xB701
LOOPBACK_HEX = f"{struct.unpack('=I', socket.inet_aton('127.0.0.1'))[0]:08X}"


def open_verified_dir(expected):
    fd = os.open(expected, os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW)
    try:
        real = os.path.realpath(f"/proc/self/fd/{fd}")
        if real != expected:
            raise RuntimeError(f"directory {expected!r} resolved to {real!r}")
        return fd
    except Exception:
        os.close(fd)
        raise


class BoundedLog:
    def __init__(self, path, max_bytes):
        self.path = path
        self.max_bytes = max(4096, int(max_bytes))
        self.lock = threading.Lock()
        parent = os.path.dirname(path)
        if parent:
            os.makedirs(parent, mode=0o700, exist_ok=True)
        with self.lock:
            fd = os.open(path, os.O_WRONLY | os.O_CREAT | os.O_APPEND, 0o600)
            os.close(fd)

    def append(self, data):
        if not data:
            return
        if isinstance(data, str):
            data = data.encode("utf-8", errors="replace")
        with self.lock:
            try:
                cur_size = os.path.getsize(self.path)
            except OSError:
                cur_size = 0
            if cur_size + len(data) <= self.max_bytes:
                with open(self.path, "ab") as f:
                    f.write(data)
                return
            keep = self.max_bytes // 2
            existing = b""
            try:
                with open(self.path, "rb") as f:
                    if cur_size > keep:
                        f.seek(cur_size - keep)
                    existing = f.read()
            except OSError:
                existing = b""
            combined = existing + data
            if len(combined) > self.max_bytes:
                combined = combined[-self.max_bytes :]
                nl = combined.find(b"\n")
                if 0 <= nl < len(combined) - 1:
                    combined = combined[nl + 1 :]
            tmp = f"{self.path}.tmp"
            with open(tmp, "wb") as f:
                f.write(combined)
            os.chmod(tmp, 0o600)
            os.replace(tmp, self.path)


def read_all(fd):
    chunks = []
    while True:
        buf = os.read(fd, 4096)
        if not buf:
            break
        chunks.append(buf)
    return b"".join(chunks)


def build_bwrap_cmd(cfg, app_fd, scr_fd, info_fd):
    cmd = [
        cfg["bwrap_path"],
        "--unshare-all",
        "--new-session",
        "--die-with-parent",
        "--clearenv",
        "--ro-bind",
        "/usr",
        "/usr",
    ]
    for sys_path in ("/bin", "/lib", "/lib64", "/sbin"):
        if os.path.islink(sys_path):
            cmd.extend(["--symlink", os.readlink(sys_path), sys_path])
        elif os.path.isdir(sys_path):
            cmd.extend(["--ro-bind", sys_path, sys_path])
    cmd.extend(["--dir", "/etc"])
    for etc_path in (
        "/etc/ld.so.cache",
        "/etc/ssl",
        "/etc/ca-certificates",
        "/etc/ca-certificates.conf",
        "/etc/pki",
        "/etc/nsswitch.conf",
        "/etc/localtime",
        "/etc/alternatives",
    ):
        if os.path.exists(etc_path):
            cmd.extend(["--ro-bind-try", etc_path, etc_path])
    cmd.extend([
        "--ro-bind", cfg["resolv_conf"], "/etc/resolv.conf",
        "--ro-bind", cfg["hosts_file"], "/etc/hosts",
        "--ro-bind", cfg["command_script"], "/.renoa-cmd.sh",
        "--proc", "/proc",
        "--dev", "/dev",
        "--bind", f"/proc/self/fd/{app_fd}", "/app",
        "--bind", f"/proc/self/fd/{scr_fd}", "/tmp",
        "--remount-ro", "/",
        "--chdir", "/app",
        "--setenv", "PATH", cfg["path_env"],
        "--setenv", "HOME", "/tmp",
        "--setenv", "TMPDIR", "/tmp",
        "--setenv", "PORT", str(cfg["port"]),
        "--setenv", "BASE_PATH", cfg["base_path"],
        "--info-fd", str(info_fd),
        "--",
        "/bin/sh",
        "/.renoa-cmd.sh",
    ])
    return cmd


def scan_sandbox_listeners(child_pid, target_port):
    found_target = False
    for table_name, is_v6 in (("tcp", False), ("tcp6", True)):
        path = f"/proc/{child_pid}/net/{table_name}"
        try:
            with open(path, "r", encoding="utf-8") as f:
                lines = f.read().splitlines()
        except OSError as exc:
            return (False, f"cannot read {path}: {exc}")
        for line in lines[1:]:
            fields = line.split()
            if len(fields) < 4 or fields[3] != "0A":
                continue
            local = fields[1]
            if ":" not in local:
                continue
            ip_hex, port_hex = local.split(":", 1)
            try:
                port = int(port_hex, 16)
            except ValueError:
                continue
            if is_v6:
                return (
                    False,
                    f"disallowed IPv6 listening socket [{ip_hex}]:{port}; live apps must bind 127.0.0.1 only.",
                )
            if ip_hex.upper() != LOOPBACK_HEX:
                return (
                    False,
                    f"disallowed non-loopback listening socket {ip_hex}:{port}; live apps must bind 127.0.0.1 only.",
                )
            if port != target_port:
                return (
                    False,
                    f"disallowed listening port {port} (expected {target_port}); live apps must bind 127.0.0.1 only on $PORT.",
                )
            found_target = True
    return (found_target, None)


def relay_connection(client_fd, port):
    client = socket.socket(fileno=client_fd)
    upstream = None
    sel = selectors.DefaultSelector()
    try:
        upstream = socket.create_connection(("127.0.0.1", port), timeout=5.0)
        client.setblocking(False)
        upstream.setblocking(False)
        bufs = {client: bytearray(), upstream: bytearray()}
        peer = {client: upstream, upstream: client}
        eof_in = {client: False, upstream: False}
        sel.register(client, selectors.EVENT_READ)
        sel.register(upstream, selectors.EVENT_READ)
        while not (eof_in[client] and eof_in[upstream] and not bufs[client] and not bufs[upstream]):
            for key, events in sel.select(timeout=5.0):
                sock = key.fileobj
                other = peer[sock]
                if events & selectors.EVENT_READ:
                    try:
                        chunk = sock.recv(65536)
                    except BlockingIOError:
                        chunk = None
                    except OSError:
                        chunk = b""
                    if chunk == b"":
                        eof_in[sock] = True
                        if not bufs[other]:
                            try:
                                other.shutdown(socket.SHUT_WR)
                            except OSError:
                                pass
                    elif chunk:
                        bufs[other].extend(chunk)
                if (events & selectors.EVENT_WRITE) and bufs[sock]:
                    try:
                        sent = sock.send(bufs[sock])
                        del bufs[sock][:sent]
                    except BlockingIOError:
                        pass
                    except OSError:
                        bufs[sock].clear()
                        eof_in[client] = eof_in[upstream] = True
                        break
                    if eof_in[other] and not bufs[sock]:
                        try:
                            sock.shutdown(socket.SHUT_WR)
                        except OSError:
                            pass
                for s in (client, upstream):
                    mask = 0
                    if not eof_in[s]:
                        mask |= selectors.EVENT_READ
                    if bufs[s]:
                        mask |= selectors.EVENT_WRITE
                    if mask:
                        sel.modify(s, mask)
                    else:
                        try:
                            sel.unregister(s)
                        except KeyError:
                            pass
    except Exception:
        pass
    finally:
        sel.close()
        for s in (client, upstream):
            if s is not None:
                try:
                    s.close()
                except OSError:
                    pass


def run_bridge_child(c_sock, ufd, nfd, port):
    os.setns(ufd, os.CLONE_NEWUSER)
    os.setns(nfd, os.CLONE_NEWNET)
    os.close(ufd)
    os.close(nfd)
    while True:
        try:
            data, fds, _, _ = socket.recv_fds(c_sock, 16, 1)
        except OSError:
            break
        if not fds or not data:
            for fd in fds:
                os.close(fd)
            break
        t = threading.Thread(target=relay_connection, args=(fds[0], port), daemon=True)
        t.start()
    c_sock.close()


def main():
    if len(sys.argv) != 2:
        sys.exit(2)
    with open(sys.argv[1], "r", encoding="utf-8") as f:
        cfg = json.load(f)

    log = BoundedLog(cfg["log_file"], cfg["max_log_bytes"])
    port = int(cfg["port"])
    sandbox_state_path = cfg["sandbox_state"]
    stop_flag = {"stop": False}

    def handle_sig(_signum, _frame):
        stop_flag["stop"] = True

    signal.signal(signal.SIGTERM, handle_sig)
    signal.signal(signal.SIGINT, handle_sig)

    bw = None
    slirp = None
    srv = None
    p_sock = None
    bridge_pid = None
    pump_thread = None

    try:
        app_fd = open_verified_dir(cfg["app_dir"])
        scr_fd = open_verified_dir(cfg["scratch_dir"])
        r_info, w_info = os.pipe()
        cmd = build_bwrap_cmd(cfg, app_fd, scr_fd, w_info)
        bw = subprocess.Popen(
            cmd,
            pass_fds=(w_info, app_fd, scr_fd),
            stdout=subprocess.PIPE,
            stderr=subprocess.STDOUT,
        )
        os.close(w_info)
        os.close(app_fd)
        os.close(scr_fd)

        def pump_logs():
            try:
                while True:
                    chunk = bw.stdout.read1(8192) if hasattr(bw.stdout, "read1") else bw.stdout.read(4096)
                    if not chunk:
                        break
                    log.append(chunk)
            except Exception:
                pass

        pump_thread = threading.Thread(target=pump_logs, daemon=True)
        pump_thread.start()

        info_raw = read_all(r_info)
        os.close(r_info)
        if not info_raw:
            log.append("[renoa-sandbox] bwrap exited before reporting child pid\n")
            bw.wait()
            sys.exit(bw.returncode or 1)
        child_pid = int(json.loads(info_raw.decode("utf-8"))["child-pid"])

        for _ in range(100):
            try:
                with open(f"/proc/{child_pid}/uid_map", "r", encoding="utf-8") as um:
                    if um.read().strip():
                        break
            except OSError:
                pass
            time.sleep(0.01)

        nfd = os.open(f"/proc/{child_pid}/ns/net", os.O_RDONLY)
        ufd = fcntl.ioctl(nfd, NS_GET_USERNS)
        try:
            r_ready, w_ready = os.pipe()
            slirp = subprocess.Popen(
                [
                    cfg["slirp4netns_path"],
                    "--configure",
                    "--mtu=65520",
                    "--disable-host-loopback",
                    "--enable-seccomp",
                    f"--userns-path=/proc/self/fd/{ufd}",
                    f"--ready-fd={w_ready}",
                    str(child_pid),
                    "tap0",
                ],
                pass_fds=(w_ready, ufd),
                stdout=subprocess.DEVNULL,
                stderr=subprocess.PIPE,
            )
            os.close(w_ready)
            ready_r, _, _ = select.select([r_ready], [], [], 5.0)
            rb = os.read(r_ready, 16) if ready_r else b""
            os.close(r_ready)
            if rb != b"1" or slirp.poll() is not None:
                err = slirp.stderr.read().decode("utf-8", errors="replace") if slirp.stderr else ""
                log.append(f"[renoa-sandbox] slirp4netns failed: {err}\n")
                sys.exit(1)

            state_tmp = f"{sandbox_state_path}.tmp"
            with open(state_tmp, "w", encoding="utf-8") as sf:
                json.dump({"child_pid": child_pid, "port": port}, sf)
            os.chmod(state_tmp, 0o600)
            os.replace(state_tmp, sandbox_state_path)

            startup_deadline = time.monotonic() + float(cfg.get("startup_timeout_secs", 15.0))
            while not stop_flag["stop"]:
                if bw.poll() is not None or slirp.poll() is not None:
                    sys.exit(bw.returncode or 1)
                ready, violation = scan_sandbox_listeners(child_pid, port)
                if violation:
                    log.append(f"[renoa-sandbox] {violation}\n")
                    sys.exit(1)
                if ready:
                    break
                if time.monotonic() >= startup_deadline:
                    log.append(f"[renoa-sandbox] timed out waiting for 127.0.0.1:{port}\n")
                    sys.exit(1)
                time.sleep(0.05)

            if stop_flag["stop"]:
                sys.exit(0)

            p_sock, c_sock = socket.socketpair(socket.AF_UNIX, socket.SOCK_DGRAM)
            pid = os.fork()
            if pid == 0:
                p_sock.close()
                try:
                    run_bridge_child(c_sock, ufd, nfd, port)
                except Exception:
                    os._exit(1)
                os._exit(0)
            bridge_pid = pid
            c_sock.close()
        finally:
            os.close(ufd)
            os.close(nfd)

        srv = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
        srv.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
        srv.bind(("127.0.0.1", port))
        srv.listen(128)
        srv.setblocking(False)

        while not stop_flag["stop"]:
            if bw.poll() is not None or slirp.poll() is not None:
                break
            _, violation = scan_sandbox_listeners(child_pid, port)
            if violation:
                log.append(f"[renoa-sandbox] {violation}\n")
                sys.exit(1)
            readable, _, _ = select.select([srv], [], [], 0.25)
            if readable:
                while True:
                    try:
                        client, _ = srv.accept()
                    except BlockingIOError:
                        break
                    try:
                        socket.send_fds(p_sock, [b"1"], [client.fileno()])
                    finally:
                        client.close()

        if stop_flag["stop"]:
            sys.exit(0)
        rc = bw.poll()
        if rc is not None:
            sys.exit(rc if rc != 0 else 1)
        sys.exit(1)
    except SystemExit:
        raise
    except Exception as exc:
        log.append(f"[renoa-sandbox] fatal: {exc}\n")
        sys.exit(1)
    finally:
        if srv is not None:
            try:
                srv.close()
            except OSError:
                pass
        if p_sock is not None:
            try:
                socket.send_fds(p_sock, [b""], [])
            except OSError:
                pass
            try:
                p_sock.close()
            except OSError:
                pass
        if bridge_pid is not None:
            try:
                os.kill(bridge_pid, signal.SIGTERM)
                os.waitpid(bridge_pid, 0)
            except OSError:
                pass
        for proc in (bw, slirp):
            if proc is not None and proc.poll() is None:
                try:
                    proc.terminate()
                    proc.wait(timeout=2)
                except Exception:
                    try:
                        proc.kill()
                        proc.wait(timeout=1)
                    except Exception:
                        pass
        if pump_thread is not None:
            pump_thread.join(timeout=1)
        try:
            os.remove(sandbox_state_path)
        except OSError:
            pass


if __name__ == "__main__":
    main()
