//! Port allocation, `127.0.0.1`-only listener verification, and HTTP readiness probes.

use std::{
    collections::HashSet,
    fs,
    net::{Ipv4Addr, SocketAddr, TcpListener},
    path::Path,
    sync::{LazyLock, Mutex},
    time::Duration,
};

use tokio::{
    io::{AsyncReadExt as _, AsyncWriteExt as _},
    net::TcpStream,
    time::{Instant, sleep, timeout},
};

use super::{AppError, STARTUP_TIMEOUT};
use crate::apps::supervisor::{self, UnitStatus};

pub(super) const PORT_RANGE_START: u16 = 20_000;
pub(super) const PORT_RANGE_END: u16 = 29_999;
const PROBE_INTERVAL: Duration = Duration::from_millis(100);
const HTTP_IO_TIMEOUT: Duration = Duration::from_secs(1);

static RESERVED_PORTS: LazyLock<Mutex<HashSet<u16>>> = LazyLock::new(|| Mutex::new(HashSet::new()));

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum ListenerViolation {
    NonLoopback(String),
    Unreadable(String),
}

impl ListenerViolation {
    pub(super) fn message(&self) -> String {
        match self {
            Self::NonLoopback(addr) => {
                format!("App bound {addr}; live apps must bind 127.0.0.1 only.")
            }
            Self::Unreadable(reason) => {
                format!("Cannot verify loopback listener: {reason}")
            }
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
    let hash = seed_name.bytes().fold(0_u32, |acc, b| {
        acc.wrapping_mul(31).wrapping_add(u32::from(b))
    });
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

/// Inspects `/proc/net/tcp` and `/proc/net/tcp6` to ensure any listening
/// socket for `port` binds strictly to `127.0.0.1` and fails closed if the
/// tables are unreadable.
pub(super) fn check_loopback_only_listener(port: u16) -> Result<bool, ListenerViolation> {
    let v4 = fs::read_to_string("/proc/net/tcp").map_err(|err| {
        ListenerViolation::Unreadable(format!("cannot read /proc/net/tcp: {err}"))
    })?;
    let mut found_loopback = check_proc_net_table(&v4, port, false)?;
    let tcp6_path = Path::new("/proc/net/tcp6");
    if tcp6_path.exists() {
        let v6 = fs::read_to_string(tcp6_path).map_err(|err| {
            ListenerViolation::Unreadable(format!("cannot read /proc/net/tcp6: {err}"))
        })?;
        found_loopback |= check_proc_net_table(&v6, port, true)?;
    }
    Ok(found_loopback)
}

fn check_proc_net_table(
    contents: &str,
    target_port: u16,
    is_v6: bool,
) -> Result<bool, ListenerViolation> {
    const TCP_LISTEN: &str = "0A";
    let mut found_loopback = false;

    for line in contents.lines().skip(1) {
        let mut fields = line.split_whitespace();
        let Some(_slot) = fields.next() else {
            continue;
        };
        let Some(local_address) = fields.next() else {
            continue;
        };
        let Some(_rem_address) = fields.next() else {
            continue;
        };
        let Some(state) = fields.next() else {
            continue;
        };
        if state != TCP_LISTEN {
            continue;
        }
        let Some((ip_hex, port_hex)) = local_address.split_once(':') else {
            continue;
        };
        let Ok(port) = u16::from_str_radix(port_hex, 16) else {
            continue;
        };
        if port != target_port {
            continue;
        }

        if is_v6 {
            // Only IPv4-mapped 127.0.0.1 (`::ffff:127.0.0.1`) is loopback v4.
            if !is_v4_mapped_loopback_hex(ip_hex) {
                return Err(ListenerViolation::NonLoopback(format!(
                    "[v6:{ip_hex}]:{port}"
                )));
            }
            found_loopback = true;
        } else {
            let Ok(raw) = u32::from_str_radix(ip_hex, 16) else {
                return Err(ListenerViolation::NonLoopback(format!("{ip_hex}:{port}")));
            };
            let ip = Ipv4Addr::from(raw.to_ne_bytes());
            if ip != Ipv4Addr::LOCALHOST {
                return Err(ListenerViolation::NonLoopback(format!("{ip}:{port}")));
            }
            found_loopback = true;
        }
    }

    Ok(found_loopback)
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

/// Waits until `unit` answers HTTP on `127.0.0.1:port`, aborts immediately if
/// it enters a non-loopback bind or crash-loop failure, or times out.
pub(super) async fn wait_until_healthy(
    unit: &str,
    port: u16,
    log_file: &std::path::Path,
    startup_timeout: Duration,
) -> Result<(), String> {
    let deadline = Instant::now()
        + if startup_timeout.is_zero() {
            STARTUP_TIMEOUT
        } else {
            startup_timeout
        };

    loop {
        if let Err(violation) = check_loopback_only_listener(port) {
            return Err(violation.message());
        }

        match supervisor::unit_status(unit).await {
            Ok(UnitStatus::Active) => {}
            Ok(UnitStatus::Failed { detail }) => {
                let tail = supervisor::read_log_tail(log_file);
                return Err(format_failure(&detail, &tail));
            }
            Ok(UnitStatus::Inactive) => {
                let tail = supervisor::read_log_tail(log_file);
                return Err(format_failure(
                    "app exited before answering HTTP on its port",
                    &tail,
                ));
            }
            Err(err) => {
                return Err(format!("could not inspect app unit state: {err}"));
            }
        }

        if probe_http(port).await {
            match check_loopback_only_listener(port) {
                Ok(true) => return Ok(()),
                Ok(false) => {}
                Err(violation) => return Err(violation.message()),
            }
        }

        if Instant::now() >= deadline {
            let tail = supervisor::read_log_tail(log_file);
            return Err(format_failure(
                &format!(
                    "app did not answer HTTP on 127.0.0.1:{port} within {}s",
                    startup_timeout.as_secs_f32()
                ),
                &tail,
            ));
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
