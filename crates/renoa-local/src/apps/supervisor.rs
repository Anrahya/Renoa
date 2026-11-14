//! Systemd `--user` transient unit supervision + `bubblewrap` sandboxing.

use std::{
    ffi::OsString,
    fs, io,
    path::{Path, PathBuf},
    process::Stdio,
};

use nix::unistd::Uid;
use sha2::{Digest as _, Sha256};
use tokio::process::Command;

use super::{
    AppError, CPU_QUOTA, CRASH_BURST, CRASH_INTERVAL_SECS, LOG_TAIL_BYTES, LOG_TAIL_LINES,
    MEMORY_MAX,
};

const BWRAP_PATH: &str = "/usr/bin/bwrap";
const SANDBOX_PATH: &str = "/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin";
const UNIT_PREFIX: &str = "renoa-app-";

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum UnitStatus {
    Active,
    Failed { detail: String },
    Inactive,
}

#[derive(Debug, Clone)]
pub(super) struct StartUnitSpec {
    pub unit: String,
    pub app_name: String,
    pub app_dir: PathBuf,
    pub scratch_dir: PathBuf,
    pub log_file: PathBuf,
    pub command: String,
    pub port: u16,
    pub base_path: String,
}

pub(super) fn home_hash(database: &Path) -> String {
    let canonical = fs::canonicalize(database).unwrap_or_else(|_| database.to_path_buf());
    let mut hasher = Sha256::new();
    hasher.update(canonical.as_os_str().as_encoded_bytes());
    let digest = hasher.finalize();
    let mut hex = String::with_capacity(12);
    for byte in &digest[..6] {
        use std::fmt::Write as _;
        let _ = write!(&mut hex, "{byte:02x}");
    }
    hex
}

pub(super) fn unit_name(database: &Path, app_name: &str) -> String {
    format!("{UNIT_PREFIX}{}-{app_name}.service", home_hash(database))
}

pub(super) fn unit_prefix_for(database: &Path) -> String {
    format!("{UNIT_PREFIX}{}-", home_hash(database))
}

fn user_runtime_env() -> Vec<(&'static str, OsString)> {
    let uid = Uid::effective().as_raw();
    let mut vars = Vec::new();
    let runtime_dir = std::env::var_os("XDG_RUNTIME_DIR")
        .filter(|v| !v.is_empty())
        .unwrap_or_else(|| OsString::from(format!("/run/user/{uid}")));
    if std::env::var_os("DBUS_SESSION_BUS_ADDRESS")
        .as_ref()
        .is_none_or(|v| v.is_empty())
    {
        let bus = Path::new(&runtime_dir).join("bus");
        if bus.exists() {
            vars.push((
                "DBUS_SESSION_BUS_ADDRESS",
                OsString::from(format!("unix:path={}", bus.display())),
            ));
        }
    }
    vars.push(("XDG_RUNTIME_DIR", runtime_dir));
    vars
}

fn user_command(program: &str) -> Command {
    let mut cmd = Command::new(program);
    cmd.stdin(Stdio::null());
    for (k, v) in user_runtime_env() {
        cmd.env(k, v);
    }
    cmd
}

/// Checks that `bwrap` and `systemd-run --user` (`user@<uid>.service`) are
/// available before mutating any disk state or database rows.
pub(super) async fn preflight() -> Result<(), AppError> {
    if !Path::new(BWRAP_PATH).is_file() {
        return Err(AppError::SupervisorUnavailable(format!(
            "`{BWRAP_PATH}` is not installed"
        )));
    }
    let output = user_command("systemctl")
        .args(["--user", "is-system-running"])
        .output()
        .await
        .map_err(|err| {
            AppError::SupervisorUnavailable(format!("cannot run `systemctl --user`: {err}"))
        })?;
    let stdout = String::from_utf8_lossy(&output.stdout).trim().to_owned();
    let stderr = String::from_utf8_lossy(&output.stderr).trim().to_owned();
    if matches!(stdout.as_str(), "running" | "degraded" | "starting") {
        return Ok(());
    }
    Err(AppError::SupervisorUnavailable(format!(
        "`systemctl --user` is not ready (status=`{stdout}`, stderr=`{stderr}`)"
    )))
}

fn append_system_ro_mounts(args: &mut Vec<OsString>) {
    args.extend([
        OsString::from("--ro-bind"),
        OsString::from("/usr"),
        OsString::from("/usr"),
    ]);

    for sys_path in ["/bin", "/lib", "/lib64", "/sbin"] {
        let p = Path::new(sys_path);
        match fs::symlink_metadata(p) {
            Ok(meta) if meta.file_type().is_symlink() => {
                if let Ok(target) = fs::read_link(p) {
                    args.extend([
                        OsString::from("--symlink"),
                        target.into_os_string(),
                        OsString::from(sys_path),
                    ]);
                }
            }
            Ok(meta) if meta.is_dir() => {
                args.extend([
                    OsString::from("--ro-bind"),
                    OsString::from(sys_path),
                    OsString::from(sys_path),
                ]);
            }
            _ => {}
        }
    }

    for extra_ro in ["/etc", "/run/systemd/resolve"] {
        if Path::new(extra_ro).is_dir() {
            args.extend([
                OsString::from("--ro-bind"),
                OsString::from(extra_ro),
                OsString::from(extra_ro),
            ]);
        }
    }
}

/// Starts a transient `systemd --user` unit running `bwrap` around `command`.
pub(super) async fn start_unit(spec: &StartUnitSpec) -> Result<(), AppError> {
    let _ = stop_unit(&spec.unit).await;

    let mut args: Vec<OsString> = vec![
        OsString::from("--user"),
        OsString::from("--quiet"),
        OsString::from("--unit"),
        OsString::from(&spec.unit),
        OsString::from("--description"),
        OsString::from(format!("Renoa live app {}", spec.app_name)),
        OsString::from("-p"),
        OsString::from("Type=exec"),
        OsString::from("-p"),
        OsString::from("Restart=on-failure"),
        OsString::from("-p"),
        OsString::from("RestartSec=200ms"),
        OsString::from("-p"),
        OsString::from(format!("StartLimitIntervalSec={CRASH_INTERVAL_SECS}s")),
        OsString::from("-p"),
        OsString::from(format!("StartLimitBurst={CRASH_BURST}")),
        OsString::from("-p"),
        OsString::from(format!("MemoryMax={MEMORY_MAX}")),
        OsString::from("-p"),
        OsString::from(format!("CPUQuota={CPU_QUOTA}")),
        OsString::from("-p"),
        OsString::from("TimeoutStopSec=5s"),
        OsString::from("-p"),
        OsString::from("KillMode=control-group"),
        OsString::from("-p"),
        OsString::from(format!("StandardOutput=append:{}", spec.log_file.display())),
        OsString::from("-p"),
        OsString::from(format!("StandardError=append:{}", spec.log_file.display())),
        OsString::from("--"),
        OsString::from(BWRAP_PATH),
        OsString::from("--unshare-all"),
        OsString::from("--share-net"),
        OsString::from("--die-with-parent"),
        OsString::from("--new-session"),
    ];

    append_system_ro_mounts(&mut args);

    args.extend([
        OsString::from("--dev"),
        OsString::from("/dev"),
        OsString::from("--proc"),
        OsString::from("/proc"),
        OsString::from("--bind"),
        spec.app_dir.as_os_str().to_os_string(),
        OsString::from("/app"),
        OsString::from("--bind"),
        spec.scratch_dir.as_os_str().to_os_string(),
        OsString::from("/tmp"),
        OsString::from("--remount-ro"),
        OsString::from("/"),
        OsString::from("--chdir"),
        OsString::from("/app"),
        OsString::from("--clearenv"),
        OsString::from("--setenv"),
        OsString::from("PATH"),
        OsString::from(SANDBOX_PATH),
        OsString::from("--setenv"),
        OsString::from("HOME"),
        OsString::from("/tmp"),
        OsString::from("--setenv"),
        OsString::from("TMPDIR"),
        OsString::from("/tmp"),
        OsString::from("--setenv"),
        OsString::from("PORT"),
        OsString::from(spec.port.to_string()),
        OsString::from("--setenv"),
        OsString::from("BASE_PATH"),
        OsString::from(&spec.base_path),
        OsString::from("--setenv"),
        OsString::from("HOST"),
        OsString::from("127.0.0.1"),
        OsString::from("--setenv"),
        OsString::from("HOSTNAME"),
        OsString::from("127.0.0.1"),
        OsString::from("--setenv"),
        OsString::from("BIND"),
        OsString::from("127.0.0.1"),
        OsString::from("--"),
        OsString::from("/bin/sh"),
        OsString::from("-c"),
        OsString::from(&spec.command),
    ]);

    let output = user_command("systemd-run")
        .args(&args)
        .output()
        .await
        .map_err(|err| {
            AppError::SupervisorUnavailable(format!("cannot run `systemd-run`: {err}"))
        })?;

    if output.status.success() {
        return Ok(());
    }
    let stderr = String::from_utf8_lossy(&output.stderr).trim().to_owned();
    Err(AppError::SupervisorUnavailable(format!(
        "systemd-run failed for `{}`: {stderr}",
        spec.unit
    )))
}

pub(super) async fn stop_unit(unit: &str) -> Result<(), AppError> {
    let stop_out = user_command("systemctl")
        .args(["--user", "stop", unit])
        .output()
        .await
        .map_err(|err| AppError::SupervisorUnavailable(format!("cannot stop `{unit}`: {err}")))?;
    let _ = user_command("systemctl")
        .args(["--user", "reset-failed", unit])
        .output()
        .await;
    if stop_out.status.success() {
        return Ok(());
    }
    let stderr = String::from_utf8_lossy(&stop_out.stderr);
    let lower = stderr.to_ascii_lowercase();
    if lower.contains("not loaded") || lower.contains("no such") {
        return Ok(());
    }
    Err(AppError::SupervisorUnavailable(format!(
        "systemctl stop failed for `{unit}`: {}",
        stderr.trim()
    )))
}

pub(super) async fn unit_status(unit: &str) -> Result<UnitStatus, io::Error> {
    let output = user_command("systemctl")
        .args([
            "--user",
            "show",
            "-p",
            "LoadState",
            "-p",
            "ActiveState",
            "-p",
            "SubState",
            "-p",
            "Result",
            "-p",
            "ExecMainStatus",
            unit,
        ])
        .output()
        .await?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(io::Error::other(format!(
            "systemctl show `{unit}` failed: {}",
            stderr.trim()
        )));
    }
    let stdout = String::from_utf8_lossy(&output.stdout);
    let mut load_state = "";
    let mut active_state = "";
    let mut sub_state = "";
    let mut result = "";
    let mut exec_status = "";
    for line in stdout.lines() {
        if let Some((k, v)) = line.split_once('=') {
            match k {
                "LoadState" => load_state = v,
                "ActiveState" => active_state = v,
                "SubState" => sub_state = v,
                "Result" => result = v,
                "ExecMainStatus" => exec_status = v,
                _ => {}
            }
        }
    }

    if load_state == "not-found" || active_state == "inactive" || active_state.is_empty() {
        return Ok(UnitStatus::Inactive);
    }
    if active_state == "failed" {
        let detail = if result.is_empty() {
            format!("unit failed (exit status {exec_status})")
        } else {
            format!("crash loop / failure ({result}, exit status {exec_status})")
        };
        return Ok(UnitStatus::Failed { detail });
    }
    if matches!(active_state, "active" | "activating" | "reloading") && sub_state != "failed" {
        return Ok(UnitStatus::Active);
    }
    Ok(UnitStatus::Inactive)
}

pub(super) async fn list_home_units(database: &Path) -> Result<Vec<String>, io::Error> {
    let pattern = format!("{}*.service", unit_prefix_for(database));
    let output = user_command("systemctl")
        .args([
            "--user",
            "list-units",
            "--all",
            "--plain",
            "--no-legend",
            &pattern,
        ])
        .output()
        .await?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(io::Error::other(format!(
            "systemctl list-units failed: {}",
            stderr.trim()
        )));
    }
    let stdout = String::from_utf8_lossy(&output.stdout);
    Ok(stdout
        .lines()
        .filter_map(|line| line.split_whitespace().next().map(str::to_owned))
        .collect())
}

pub(super) fn read_log_tail(log_file: &Path) -> String {
    use std::io::{Read as _, Seek as _, SeekFrom};
    let Ok(mut file) = fs::File::open(log_file) else {
        return String::new();
    };
    let Ok(len) = file.metadata().map(|m| m.len()) else {
        return String::new();
    };
    let limit = LOG_TAIL_BYTES as u64;
    let start = len.saturating_sub(limit);
    if file.seek(SeekFrom::Start(start)).is_err() {
        return String::new();
    }
    let mut bytes = Vec::with_capacity(usize::try_from(len - start).unwrap_or(LOG_TAIL_BYTES));
    if file.take(limit).read_to_end(&mut bytes).is_err() {
        return String::new();
    }
    let text = String::from_utf8_lossy(&bytes);
    let mut lines: Vec<&str> = text.lines().collect();
    if lines.len() > LOG_TAIL_LINES {
        lines = lines.split_off(lines.len() - LOG_TAIL_LINES);
    }
    lines.join("\n")
}

fn sync_systemctl() -> std::process::Command {
    let mut cmd = std::process::Command::new("systemctl");
    cmd.stdin(Stdio::null());
    for (k, v) in user_runtime_env() {
        cmd.env(k, v);
    }
    cmd
}

/// Synchronously stops any `renoa-app-<hash12>-*.service` units for `database`
/// during host reset.
pub(crate) fn stop_units_sync(database: &Path) {
    let pattern = format!("{}*.service", unit_prefix_for(database));
    let Ok(output) = sync_systemctl()
        .args([
            "--user",
            "list-units",
            "--all",
            "--plain",
            "--no-legend",
            &pattern,
        ])
        .output()
    else {
        return;
    };
    if !output.status.success() {
        return;
    }
    let stdout = String::from_utf8_lossy(&output.stdout);
    for line in stdout.lines() {
        let Some(unit) = line.split_whitespace().next() else {
            continue;
        };
        let _ = sync_systemctl().args(["--user", "stop", unit]).output();
        let _ = sync_systemctl()
            .args(["--user", "reset-failed", unit])
            .output();
    }
}

pub(crate) fn stop_unit_sync(database: &Path, app_name: &str) {
    let unit = unit_name(database, app_name);
    for args in [["--user", "stop", &unit], ["--user", "reset-failed", &unit]] {
        let _ = sync_systemctl().args(args).output();
    }
}

/// RAII drop guard stopping a transient unit if startup is cancelled or fails
/// before the database row commits.
pub(super) struct UnitCleanup {
    database: PathBuf,
    app_name: String,
    armed: bool,
}

impl UnitCleanup {
    pub(super) fn armed(database: &Path, app_name: &str) -> Self {
        Self {
            database: database.to_path_buf(),
            app_name: app_name.to_owned(),
            armed: true,
        }
    }

    pub(super) fn disarm(&mut self) {
        self.armed = false;
    }
}

impl Drop for UnitCleanup {
    fn drop(&mut self) {
        if self.armed {
            stop_unit_sync(&self.database, &self.app_name);
        }
    }
}

#[cfg(test)]
pub(crate) fn unit_main_pid_sync(database: &Path, app_name: &str) -> u32 {
    let unit = unit_name(database, app_name);
    let mut cmd = std::process::Command::new("systemctl");
    cmd.stdin(Stdio::null());
    for (k, v) in user_runtime_env() {
        cmd.env(k, v);
    }
    let out = cmd
        .args(["--user", "show", "-p", "MainPID", "--value", &unit])
        .output()
        .expect("systemctl show MainPID");
    String::from_utf8_lossy(&out.stdout)
        .trim()
        .parse::<u32>()
        .unwrap_or(0)
}
