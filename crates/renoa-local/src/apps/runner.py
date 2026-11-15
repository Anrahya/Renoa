#!/usr/bin/env python3
"""Supervises one live app inside its systemd unit.

`runner.py CONFIG` starts the app under bubblewrap with no network, gives it
slirp4netns outbound networking, and serves `127.0.0.1:$PORT` on the host by
handing each accepted socket to the bridge. `runner.py --bridge CONFIG FD...`
is the bridge: it joins the sandbox's user and network namespaces, installs
an unreachable route for every CIDR in `blocked_routes`, reports when the app
accepts connections on its loopback port, and relays every connection the
runner hands it.
"""
import errno
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
CHUNK_BYTES = 64 * 1024
# A side stops being read while its peer has this much unsent data, so one
# relay buffers at most HIGH_WATER_BYTES + CHUNK_BYTES per direction.
HIGH_WATER_BYTES = 64 * 1024
ROUTES_READY = b"R"
APP_LISTENING = b"L"


def open_verified_dir(expected):
    fd = os.open(expected, os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW | os.O_CLOEXEC)
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
        os.close(os.open(path, os.O_WRONLY | os.O_CREAT | os.O_APPEND, 0o600))

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

    def pump(self, stream, prefix=b""):
        """Copies `stream` into the log until EOF so its writer never blocks."""

        def run():
            try:
                while line := stream.readline(8192):
                    self.append(prefix + line)
            except (OSError, ValueError):
                pass

        threading.Thread(target=run, daemon=True).start()


def read_all(fd):
    chunks = []
    while buf := os.read(fd, 4096):
        chunks.append(buf)
    return b"".join(chunks)


def build_bwrap_cmd(cfg, app_fd, scr_fd, info_fd, block_fd):
    # bwrap closes --bind-fd, --info-fd and --block-fd descriptors before the
    # command runs, so the app inherits only stdin, stdout and stderr.
    cmd = [cfg["bwrap_path"], "--unshare-all", "--new-session", "--die-with-parent",
           "--clearenv", "--ro-bind", "/usr", "/usr"]
    for sys_path in ("/bin", "/lib", "/lib64", "/sbin"):
        if os.path.islink(sys_path):
            cmd.extend(["--symlink", os.readlink(sys_path), sys_path])
        elif os.path.isdir(sys_path):
            cmd.extend(["--ro-bind", sys_path, sys_path])
    cmd.extend(["--dir", "/etc"])
    for etc_path in ("/etc/ld.so.cache", "/etc/ssl", "/etc/ca-certificates",
                     "/etc/ca-certificates.conf", "/etc/pki", "/etc/nsswitch.conf",
                     "/etc/localtime", "/etc/alternatives"):
        if os.path.exists(etc_path):
            cmd.extend(["--ro-bind-try", etc_path, etc_path])
    cmd.extend([
        "--ro-bind", cfg["resolv_conf"], "/etc/resolv.conf",
        "--ro-bind", cfg["hosts_file"], "/etc/hosts",
        "--ro-bind", cfg["command_script"], "/.renoa-cmd.sh",
        "--proc", "/proc",
        "--dev", "/dev",
        "--bind-fd", str(app_fd), "/app",
        "--bind-fd", str(scr_fd), "/tmp",
        "--remount-ro", "/",
        "--chdir", "/app",
        "--info-fd", str(info_fd),
        "--block-fd", str(block_fd),
    ])
    for key, value in cfg["env"].items():
        cmd.extend(["--setenv", key, value])
    cmd.extend(["--", "/bin/sh", "/.renoa-cmd.sh"])
    return cmd


def add_unreachable_routes(cidrs):
    """Adds one RTN_UNREACHABLE main-table route per IPv4 CIDR over rtnetlink."""
    nl = socket.socket(socket.AF_NETLINK, socket.SOCK_RAW, socket.NETLINK_ROUTE)
    try:
        nl.bind((0, 0))
        for seq, cidr in enumerate(cidrs, 1):
            network, prefix = cidr.split("/")
            # rtmsg: family, dst_len, src_len, tos, table=MAIN, protocol=STATIC,
            # scope=UNIVERSE, type=UNREACHABLE, flags; then RTA_DST.
            body = struct.pack("=BBBBBBBBI", socket.AF_INET, int(prefix), 0, 0, 254, 4, 0, 7, 0)
            body += struct.pack("=HH", 8, 1) + socket.inet_aton(network)
            # nlmsghdr: RTM_NEWROUTE with REQUEST | ACK | EXCL | CREATE.
            nl.send(struct.pack("=IHHII", 16 + len(body), 24, 0x1 | 0x4 | 0x200 | 0x400, seq, 0) + body)
            reply = nl.recv(4096)
            kind = struct.unpack_from("=H", reply, 4)[0]
            code = struct.unpack_from("=i", reply, 16)[0]
            if kind != 2 or code != 0:
                raise OSError(-code, f"cannot add unreachable route {cidr}: {os.strerror(-code)}")
    finally:
        nl.close()


class Relay:
    """One accepted host connection and its upstream inside the sandbox."""

    def __init__(self, client, upstream):
        self.peer = {client: upstream, upstream: client}
        self.pending = {client: bytearray(), upstream: bytearray()}
        self.eof = {client: False, upstream: False}
        self.upstream = upstream
        self.connecting = True
        self.last_active = time.monotonic()

    def interest(self, sock):
        if sock is self.upstream and self.connecting:
            return selectors.EVENT_WRITE
        mask = 0
        if not self.eof[sock] and len(self.pending[self.peer[sock]]) < HIGH_WATER_BYTES:
            mask |= selectors.EVENT_READ
        if self.pending[sock]:
            mask |= selectors.EVENT_WRITE
        return mask

    def handle(self, sock, events):
        """Moves bytes for one ready socket; returns False once the relay is done."""
        self.last_active = time.monotonic()
        if sock is self.upstream and self.connecting:
            self.connecting = False
            return sock.getsockopt(socket.SOL_SOCKET, socket.SO_ERROR) == 0
        other = self.peer[sock]
        if events & selectors.EVENT_READ:
            if chunk := sock.recv(CHUNK_BYTES):
                self.pending[other] += chunk
            else:
                self.eof[sock] = True
                if not self.pending[other]:
                    other.shutdown(socket.SHUT_WR)
        if events & selectors.EVENT_WRITE and self.pending[sock]:
            del self.pending[sock][: sock.send(self.pending[sock])]
            if self.eof[other] and not self.pending[sock]:
                sock.shutdown(socket.SHUT_WR)
        return not (all(self.eof.values()) and not any(self.pending.values()))


class Bridge:
    """Relays host connections to the app's loopback port with bounded memory."""

    def __init__(self, port, max_relays, idle_secs):
        self.port = port
        self.max_relays = max_relays
        self.idle_secs = idle_secs
        self.sel = selectors.DefaultSelector()
        self.relays = {}

    def refresh(self, relay):
        for sock in relay.peer:
            mask = relay.interest(sock)
            registered = sock in self.sel.get_map()
            if mask and registered:
                self.sel.modify(sock, mask, relay)
            elif mask:
                self.sel.register(sock, mask, relay)
            elif registered:
                self.sel.unregister(sock)

    def close(self, relay):
        for sock in relay.peer:
            self.relays.pop(sock, None)
            if sock in self.sel.get_map():
                self.sel.unregister(sock)
            sock.close()

    def accept(self, fd):
        client = socket.socket(fileno=fd)
        if len(self.relays) // 2 >= self.max_relays:
            client.close()
            return
        try:
            upstream = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
        except OSError:
            client.close()
            return
        client.setblocking(False)
        upstream.setblocking(False)
        if upstream.connect_ex(("127.0.0.1", self.port)) not in (0, errno.EINPROGRESS):
            client.close()
            upstream.close()
            return
        relay = Relay(client, upstream)
        self.relays[client] = self.relays[upstream] = relay
        self.refresh(relay)

    def serve(self, control):
        self.sel.register(control, selectors.EVENT_READ)
        while True:
            for key, events in self.sel.select(timeout=min(self.idle_secs, 5.0)):
                if key.fileobj is control:
                    data, fds, _, _ = socket.recv_fds(control, 1, 1)
                    if not data:
                        return
                    for fd in fds:
                        self.accept(fd)
                    continue
                relay = key.data
                if self.relays.get(key.fileobj) is not relay:
                    continue
                try:
                    alive = relay.handle(key.fileobj, events)
                except (BlockingIOError, InterruptedError):
                    alive = True
                except OSError:
                    alive = False
                if alive:
                    self.refresh(relay)
                else:
                    self.close(relay)
            now = time.monotonic()
            for relay in {r for r in self.relays.values() if now - r.last_active > self.idle_secs}:
                self.close(relay)


def run_bridge(cfg, control_fd, status_fd, userns_fd, netns_fd):
    os.setns(userns_fd, os.CLONE_NEWUSER)
    os.setns(netns_fd, os.CLONE_NEWNET)
    os.close(userns_fd)
    os.close(netns_fd)
    add_unreachable_routes(cfg["blocked_routes"])
    os.write(status_fd, ROUTES_READY)
    port = int(cfg["port"])
    while True:
        with socket.socket(socket.AF_INET, socket.SOCK_STREAM) as probe:
            if probe.connect_ex(("127.0.0.1", port)) == 0:
                break
        time.sleep(0.05)
    os.write(status_fd, APP_LISTENING)
    os.close(status_fd)
    bridge = Bridge(port, int(cfg["max_connections"]), float(cfg["idle_timeout_secs"]))
    bridge.serve(socket.socket(fileno=control_fd))


def wait_for(fd, expected, deadline, alive):
    while time.monotonic() < deadline and alive():
        ready, _, _ = select.select([fd], [], [], 0.05)
        if ready:
            return os.read(fd, 1) == expected
    return False


def stop_process(proc):
    if proc.poll() is not None:
        return
    proc.terminate()
    try:
        proc.wait(timeout=2)
    except subprocess.TimeoutExpired:
        proc.kill()
        proc.wait(timeout=1)


def supervise(config_path, cfg):
    log = BoundedLog(cfg["log_file"], cfg["max_log_bytes"])
    port = int(cfg["port"])
    stop = threading.Event()
    signal.signal(signal.SIGTERM, lambda *_: stop.set())
    signal.signal(signal.SIGINT, lambda *_: stop.set())
    procs = []

    def alive():
        return not stop.is_set() and all(proc.poll() is None for proc in procs)

    srv = control = None
    try:
        app_fd = open_verified_dir(cfg["app_dir"])
        scr_fd = open_verified_dir(cfg["scratch_dir"])
        info_r, info_w = os.pipe()
        block_r, block_w = os.pipe()
        bw = subprocess.Popen(build_bwrap_cmd(cfg, app_fd, scr_fd, info_w, block_r),
                              pass_fds=(app_fd, scr_fd, info_w, block_r),
                              stdin=subprocess.DEVNULL, stdout=subprocess.PIPE,
                              stderr=subprocess.STDOUT)
        procs.append(bw)
        for fd in (app_fd, scr_fd, info_w, block_r):
            os.close(fd)
        log.pump(bw.stdout)

        info_raw = read_all(info_r)
        os.close(info_r)
        if not info_raw:
            log.append("[renoa-sandbox] bwrap exited before reporting its child\n")
            return 1
        child_pid = int(json.loads(info_raw)["child-pid"])
        for _ in range(100):
            with open(f"/proc/{child_pid}/uid_map", encoding="utf-8") as uid_map:
                if uid_map.read().strip():
                    break
            time.sleep(0.01)

        netns_fd = os.open(f"/proc/{child_pid}/ns/net", os.O_RDONLY | os.O_CLOEXEC)
        userns_fd = fcntl.ioctl(netns_fd, NS_GET_USERNS)
        ready_r, ready_w = os.pipe()
        slirp = subprocess.Popen(
            [cfg["slirp4netns_path"], "--configure", "--mtu=65520", "--disable-host-loopback",
             "--enable-seccomp", f"--userns-path=/proc/self/fd/{userns_fd}",
             f"--ready-fd={ready_w}", str(child_pid), "tap0"],
            pass_fds=(ready_w, userns_fd), stdin=subprocess.DEVNULL,
            stdout=subprocess.DEVNULL, stderr=subprocess.PIPE)
        procs.append(slirp)
        os.close(ready_w)
        log.pump(slirp.stderr, b"[slirp4netns] ")
        if not wait_for(ready_r, b"1", time.monotonic() + 5.0, alive):
            log.append("[renoa-sandbox] slirp4netns did not configure the network\n")
            return 0 if stop.is_set() else 1
        os.close(ready_r)

        # SEQPACKET, not DGRAM: the bridge sees end-of-file when the runner dies.
        control, bridge_end = socket.socketpair(socket.AF_UNIX, socket.SOCK_SEQPACKET)
        status_r, status_w = os.pipe()
        fds = (bridge_end.fileno(), status_w, userns_fd, netns_fd)
        bridge = subprocess.Popen(
            [sys.executable, "-I", os.path.abspath(__file__), "--bridge", config_path,
             *map(str, fds)],
            pass_fds=fds, stdin=subprocess.DEVNULL, stdout=subprocess.DEVNULL,
            stderr=subprocess.PIPE)
        procs.append(bridge)
        log.pump(bridge.stderr, b"[renoa-bridge] ")
        bridge_end.close()
        for fd in (status_w, userns_fd, netns_fd):
            os.close(fd)

        if not wait_for(status_r, ROUTES_READY, time.monotonic() + 5.0, alive):
            log.append("[renoa-sandbox] could not restrict the sandbox network\n")
            return 0 if stop.is_set() else 1
        os.write(block_w, b"1")
        os.close(block_w)
        startup_deadline = time.monotonic() + float(cfg["startup_timeout_secs"])
        if not wait_for(status_r, APP_LISTENING, startup_deadline, alive):
            if stop.is_set():
                return 0
            log.append(f"[renoa-sandbox] app did not listen on 127.0.0.1:{port}\n")
            return 1
        os.close(status_r)

        srv = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
        srv.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
        srv.bind(("127.0.0.1", port))
        srv.listen(128)
        srv.setblocking(False)
        while alive():
            readable, _, _ = select.select([srv], [], [], 0.25)
            if not readable:
                continue
            try:
                client, _ = srv.accept()
            except BlockingIOError:
                continue
            with client:
                socket.send_fds(control, [b"1"], [client.fileno()])
        if stop.is_set():
            return 0
        log.append("[renoa-sandbox] a sandbox process exited\n")
        return bw.poll() or 1
    except Exception as exc:
        log.append(f"[renoa-sandbox] fatal: {exc}\n")
        return 1
    finally:
        if srv is not None:
            srv.close()
        if control is not None:
            control.close()
        for proc in reversed(procs):
            stop_process(proc)


def main():
    if len(sys.argv) == 7 and sys.argv[1] == "--bridge":
        with open(sys.argv[2], encoding="utf-8") as f:
            cfg = json.load(f)
        run_bridge(cfg, *map(int, sys.argv[3:]))
        return 0
    if len(sys.argv) != 2:
        return 2
    with open(sys.argv[1], encoding="utf-8") as f:
        return supervise(sys.argv[1], json.load(f))


if __name__ == "__main__":
    sys.exit(main())
