//! Port allocation, unit-scoped `127.0.0.1`-only listener verification, and
//! HTTP readiness probes.

use std::{
    collections::HashSet,
    fs,
    net::{Ipv4Addr, SocketAddr, TcpListener},
    path::Path,
    sync::{
        LazyLock, Mutex,
        atomic::{AtomicU32, Ordering},
    },
    time::Duration,
};

use tokio::{
    io::{AsyncReadExt as _, AsyncWriteExt as _},
    net::TcpStream,
    time::{Instant, sleep, timeout},
};

use super::{
    AppError, STARTUP_TIMEOUT, sandbox,
    supervisor::{self, UnitStatus},
};

pub(super) const PORT_RANGE_START: u16 = 20_000;
pub(super) const PORT_RANGE_END: u16 = 29_999;
const PROBE_INTERVAL: Duration = Duration::from_millis(100);
const HTTP_IO_TIMEOUT: Duration = Duration::from_secs(1);

static RESERVED_PORTS: LazyLock<Mutex<HashSet<u16>>> = LazyLock::new(|| Mutex::new(HashSet::new()));
static PORT_JITTER: AtomicU32 = AtomicU32::new(0);

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum ListenerViolation {
    NonLoopback(String),
    PortSquatted(u16),
    Unreadable(String),
}

impl ListenerViolation {
    pub(super) fn message(&self) -> String {
        match self {
            Self::NonLoopback(addr) => {
                format!("App bound {addr}; live apps must bind 127.0.0.1 only on $PORT.")
            }
            Self::PortSquatted(port) => {
                format!("Host port 127.0.0.1:{port} is held by a process outside the app unit.")
            }
            Self::Unreadable(reason) => {
                format!("Cannot verify loopback listener: {reason}")
            }
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum StartupError {
    Failed(String),
    TimedOut(String),
}

impl StartupError {
    pub(super) fn into_message(self) -> String {
        match self {
            Self::Failed(msg) | Self::TimedOut(msg) => msg,
        }
    }
}

/// Process-wide reservation guarding a port between `allocate_port` and
/// committing the `host_apps` catalog row.
pub(super) struct PortReservation {
    port: u16,
}

impl PortReservation {
    pub(super) const fn port(&self) -> u16 {
        self.port
    }
}

impl Drop for PortReservation {
    fn drop(&mut self) {
        let mut guard = RESERVED_PORTS
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        guard.remove(&self.port);
    }
}

/// Allocates a free localhost port in `20000..=29999` not claimed in `used` or
/// currently reserved by another in-flight startup.
pub(super) fn allocate_port(
    used: &HashSet<u16>,
    seed_name: &str,
) -> Result<PortReservation, AppError> {
    let span = u32::from(PORT_RANGE_END - PORT_RANGE_START) + 1;
    let seq = PORT_JITTER.fetch_add(1, Ordering::Relaxed);
    let mut hash = std::process::id()
        .wrapping_mul(16_777_619)
        .wrapping_add(seq.wrapping_mul(2_654_435_761));
    for byte in seed_name.bytes() {
        hash = hash.wrapping_mul(31).wrapping_add(u32::from(byte));
    }
    let start_offset = hash % span;

    let mut reserved = RESERVED_PORTS
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    for step in 0..span {
        let port_u32 = u32::from(PORT_RANGE_START) + ((start_offset + step) % span);
        let Ok(port) = u16::try_from(port_u32) else {
            continue;
        };
        if used.contains(&port) || reserved.contains(&port) {
            continue;
        }
        if TcpListener::bind((Ipv4Addr::LOCALHOST, port)).is_ok() {
            reserved.insert(port);
            return Ok(PortReservation { port });
        }
    }
    Err(AppError::NoAvailablePort)
}

fn read_cgroup(pid: u32) -> Option<String> {
    fs::read_to_string(format!("/proc/{pid}/cgroup"))
        .ok()
        .map(|s| s.trim().to_owned())
        .filter(|s| !s.is_empty())
}

fn scan_sandbox_tables(child_pid: u32, target_port: u16) -> Result<bool, ListenerViolation> {
    let v4_path = format!("/proc/{child_pid}/net/tcp");
    let v4 = fs::read_to_string(&v4_path)
        .map_err(|err| ListenerViolation::Unreadable(format!("cannot read {v4_path}: {err}")))?;
    let found_target = check_sandbox_table(&v4, target_port, false)?;

    let v6_path = format!("/proc/{child_pid}/net/tcp6");
    if Path::new(&v6_path).exists() {
        let v6 = fs::read_to_string(&v6_path).map_err(|err| {
            ListenerViolation::Unreadable(format!("cannot read {v6_path}: {err}"))
        })?;
        let _ = check_sandbox_table(&v6, target_port, true)?;
    }
    Ok(found_target)
}

fn check_sandbox_table(
    contents: &str,
    expected_port: u16,
    is_v6: bool,
) -> Result<bool, ListenerViolation> {
    const TCP_LISTEN: &str = "0A";
    let mut found_target = false;

    for line in contents.lines().skip(1) {
        let fields: Vec<&str> = line.split_whitespace().collect();
        if fields.len() < 10 || fields[3] != TCP_LISTEN {
            continue;
        }
        let Some((ip_hex, port_hex)) = fields[1].split_once(':') else {
            continue;
        };
        let Ok(port) = u16::from_str_radix(port_hex, 16) else {
            continue;
        };
        if is_v6 {
            return Err(ListenerViolation::NonLoopback(format!(
                "[v6:{ip_hex}]:{port}"
            )));
        }
        let Ok(raw) = u32::from_str_radix(ip_hex, 16) else {
            return Err(ListenerViolation::NonLoopback(format!("{ip_hex}:{port}")));
        };
        let ip = Ipv4Addr::from(raw.to_ne_bytes());
        if ip != Ipv4Addr::LOCALHOST || port != expected_port {
            return Err(ListenerViolation::NonLoopback(format!("{ip}:{port}")));
        }
        found_target = true;
    }

    Ok(found_target)
}

fn find_host_loopback_inodes(target_port: u16) -> Result<Vec<u64>, ListenerViolation> {
    const TCP_LISTEN: &str = "0A";
    let v4 = fs::read_to_string("/proc/net/tcp").map_err(|err| {
        ListenerViolation::Unreadable(format!("cannot read /proc/net/tcp: {err}"))
    })?;
    let mut inodes = Vec::new();
    for line in v4.lines().skip(1) {
        let fields: Vec<&str> = line.split_whitespace().collect();
        if fields.len() < 10 || fields[3] != TCP_LISTEN {
            continue;
        }
        let Some((ip_hex, port_hex)) = fields[1].split_once(':') else {
            continue;
        };
        let Ok(port) = u16::from_str_radix(port_hex, 16) else {
            continue;
        };
        if port != target_port {
            continue;
        }
        let Ok(raw) = u32::from_str_radix(ip_hex, 16) else {
            return Err(ListenerViolation::NonLoopback(format!("{ip_hex}:{port}")));
        };
        let ip = Ipv4Addr::from(raw.to_ne_bytes());
        if ip != Ipv4Addr::LOCALHOST {
            return Err(ListenerViolation::NonLoopback(format!("{ip}:{port}")));
        }
        if let Ok(inode) = fields[9].parse::<u64>() {
            inodes.push(inode);
        }
    }

    let v6_path = Path::new("/proc/net/tcp6");
    if v6_path.exists() {
        let v6 = fs::read_to_string(v6_path).map_err(|err| {
            ListenerViolation::Unreadable(format!("cannot read /proc/net/tcp6: {err}"))
        })?;
        for line in v6.lines().skip(1) {
            let fields: Vec<&str> = line.split_whitespace().collect();
            if fields.len() < 10 || fields[3] != TCP_LISTEN {
                continue;
            }
            let Some((ip_hex, port_hex)) = fields[1].split_once(':') else {
                continue;
            };
            let Ok(port) = u16::from_str_radix(port_hex, 16) else {
                continue;
            };
            if port != target_port {
                continue;
            }
            if !is_v4_mapped_loopback_hex(ip_hex) {
                return Err(ListenerViolation::NonLoopback(format!(
                    "[v6:{ip_hex}]:{port}"
                )));
            }
            if let Ok(inode) = fields[9].parse::<u64>() {
                inodes.push(inode);
            }
        }
    }

    Ok(inodes)
}

fn cgroup_owns_socket_inode(unit_cgroup: &str, inodes: &[u64]) -> bool {
    let Ok(proc_entries) = fs::read_dir("/proc") else {
        return false;
    };
    let targets: HashSet<String> = inodes
        .iter()
        .map(|inode| format!("socket:[{inode}]"))
        .collect();
    for entry in proc_entries.flatten() {
        let name = entry.file_name();
        let Some(pid_str) = name.to_str() else {
            continue;
        };
        let Ok(pid) = pid_str.parse::<u32>() else {
            continue;
        };
        if read_cgroup(pid).as_deref() != Some(unit_cgroup) {
            continue;
        }
        let Ok(fd_entries) = fs::read_dir(entry.path().join("fd")) else {
            continue;
        };
        for fd_entry in fd_entries.flatten() {
            if let Ok(link) = fs::read_link(fd_entry.path())
                && let Some(link_str) = link.to_str()
                && targets.contains(link_str)
            {
                return true;
            }
        }
    }
    false
}

/// Verifies both the sandbox network namespace listener and that host
/// `127.0.0.1:port` is owned by a process in `main_pid`'s cgroup.
pub(super) fn check_unit_listener(
    main_pid: u32,
    runtime_dir: &Path,
    port: u16,
) -> Result<bool, ListenerViolation> {
    if main_pid == 0 {
        return Ok(false);
    }
    let Some(unit_cgroup) = read_cgroup(main_pid) else {
        return Ok(false);
    };

    let host_inodes = find_host_loopback_inodes(port)?;
    if !host_inodes.is_empty() && !cgroup_owns_socket_inode(&unit_cgroup, &host_inodes) {
        return Err(ListenerViolation::PortSquatted(port));
    }

    let state = match sandbox::read_sandbox_state(runtime_dir) {
        Ok(Some(state)) => state,
        Ok(None) => return Ok(false),
        Err(reason) => return Err(ListenerViolation::Unreadable(reason)),
    };
    if state.port != port {
        return Err(ListenerViolation::Unreadable(format!(
            "sandbox port {} does not match assigned port {port}",
            state.port
        )));
    }
    if read_cgroup(state.child_pid).as_deref() != Some(unit_cgroup.as_str()) {
        return Ok(false);
    }
    if !scan_sandbox_tables(state.child_pid, port)? {
        return Ok(false);
    }

    Ok(!host_inodes.is_empty() && cgroup_owns_socket_inode(&unit_cgroup, &host_inodes))
}

fn is_v4_mapped_loopback_hex(ip_hex: &str) -> bool {
    if ip_hex.len() != 32 {
        return false;
    }
    let w0 = u32::from_str_radix(&ip_hex[0..8], 16).ok();
    let w1 = u32::from_str_radix(&ip_hex[8..16], 16).ok();
    let w2 = u32::from_str_radix(&ip_hex[16..24], 16).ok();
    let w3 = u32::from_str_radix(&ip_hex[24..32], 16).ok();
    let (Some(w0), Some(w1), Some(w2), Some(w3)) = (w0, w1, w2, w3) else {
        return false;
    };
    let mapped_prefix = u32::from_ne_bytes([0x00, 0x00, 0xff, 0xff]);
    w0 == 0
        && w1 == 0
        && w2 == mapped_prefix
        && Ipv4Addr::from(w3.to_ne_bytes()) == Ipv4Addr::LOCALHOST
}

/// Sends a lightweight HTTP/1.1 `GET /` probe to `127.0.0.1:port` and verifies
/// an HTTP status line is returned.
pub(super) async fn probe_http(port: u16) -> bool {
    let addr = SocketAddr::from((Ipv4Addr::LOCALHOST, port));
    let Ok(Ok(mut stream)) = timeout(HTTP_IO_TIMEOUT, TcpStream::connect(addr)).await else {
        return false;
    };
    let request = format!("GET / HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nConnection: close\r\n\r\n");
    if !matches!(
        timeout(HTTP_IO_TIMEOUT, stream.write_all(request.as_bytes())).await,
        Ok(Ok(()))
    ) {
        return false;
    }
    let mut buf = [0_u8; 256];
    let mut read_total = 0;
    while read_total < buf.len() {
        match timeout(HTTP_IO_TIMEOUT, stream.read(&mut buf[read_total..])).await {
            Ok(Ok(0) | Err(_)) | Err(_) => break,
            Ok(Ok(n)) => {
                read_total += n;
                if buf[..read_total].contains(&b'\n') {
                    break;
                }
            }
        }
    }
    let response = String::from_utf8_lossy(&buf[..read_total]);
    response.starts_with("HTTP/1.") || response.starts_with("HTTP/2")
}

/// Waits until `unit` answers HTTP on `127.0.0.1:port` inside its unit-owned
/// sandbox bridge, aborts immediately on policy violation or crash loop, or
/// returns `StartupError::TimedOut` on deadline expiry.
pub(super) async fn wait_until_healthy(
    unit: &str,
    runtime_dir: &Path,
    port: u16,
    log_file: &Path,
    startup_timeout: Duration,
) -> Result<(), StartupError> {
    let effective_timeout = if startup_timeout.is_zero() {
        STARTUP_TIMEOUT
    } else {
        startup_timeout
    };
    let deadline = Instant::now() + effective_timeout;

    loop {
        let main_pid = match supervisor::unit_status(unit).await {
            Ok(UnitStatus::Active { main_pid }) => main_pid,
            Ok(UnitStatus::Activating) => {
                if Instant::now() >= deadline {
                    let tail = supervisor::read_log_tail(log_file);
                    return Err(StartupError::TimedOut(format_failure(
                        &format!(
                            "app unit did not activate within {}s",
                            effective_timeout.as_secs_f32()
                        ),
                        &tail,
                    )));
                }
                sleep(PROBE_INTERVAL).await;
                continue;
            }
            Ok(UnitStatus::Failed { detail }) => {
                let tail = supervisor::read_log_tail(log_file);
                return Err(StartupError::Failed(format_failure(&detail, &tail)));
            }
            Ok(UnitStatus::Inactive) => {
                let tail = supervisor::read_log_tail(log_file);
                return Err(StartupError::Failed(format_failure(
                    "app exited before answering HTTP on its port",
                    &tail,
                )));
            }
            Err(err) => {
                return Err(StartupError::Failed(format!(
                    "could not inspect app unit state: {err}"
                )));
            }
        };

        match check_unit_listener(main_pid, runtime_dir, port) {
            Ok(true) => {
                if probe_http(port).await {
                    match check_unit_listener(main_pid, runtime_dir, port) {
                        Ok(true) => return Ok(()),
                        Ok(false) => {}
                        Err(violation) => return Err(StartupError::Failed(violation.message())),
                    }
                }
            }
            Ok(false) => {}
            Err(violation) => return Err(StartupError::Failed(violation.message())),
        }

        if Instant::now() >= deadline {
            let tail = supervisor::read_log_tail(log_file);
            return Err(StartupError::TimedOut(format_failure(
                &format!(
                    "app did not answer HTTP on 127.0.0.1:{port} within {}s",
                    effective_timeout.as_secs_f32()
                ),
                &tail,
            )));
        }

        sleep(PROBE_INTERVAL).await;
    }
}

fn format_failure(reason: &str, log_tail: &str) -> String {
    if log_tail.is_empty() {
        reason.to_owned()
    } else {
        format!("{reason}\n\nRecent logs:\n{log_tail}")
    }
}
