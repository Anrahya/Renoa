//! Systemd `--user` transient unit supervision backed by `sandbox`.

use std::{
    ffi::{OsStr, OsString},
    fs, io,
    os::unix::ffi::{OsStrExt as _, OsStringExt as _},
    path::{Path, PathBuf},
    process::Stdio,
};

use nix::unistd::Uid;
use sha2::{Digest as _, Sha256};
use tokio::process::Command;

use super::{
    AppError, CPU_QUOTA, CRASH_BURST, CRASH_INTERVAL_SECS, MEMORY_MAX,
    model::{APPS_SLICE, MEMORY_SWAP_MAX, SLICE_MEMORY_MAX, SLICE_TASKS_MAX, TASKS_MAX},
    sandbox::{BWRAP_PATH, PYTHON3_PATH, SLIRP4NETNS_PATH, SandboxSpec, write_runtime_artifacts},
};

pub(super) use super::sandbox::read_log_tail;

const UNIT_PREFIX: &str = "renoa-app-";

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum UnitStatus {
    Active { main_pid: u32 },
    Activating,
    Failed { detail: String },
    Inactive,
}

#[derive(Debug, Clone)]
pub(super) struct StartUnitSpec {
    pub unit: String,
    pub app_name: String,
    pub app_dir: PathBuf,
    pub scratch_dir: PathBuf,
    pub runtime_dir: PathBuf,
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

pub(super) fn staging_unit_name(database: &Path, app_name: &str, seq: u64) -> String {
    let pid = std::process::id();
    format!(
        "{UNIT_PREFIX}{}-{app_name}-stage-{pid}-{seq}.service",
        home_hash(database)
    )
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

fn escape_systemd_dollars(arg: &OsStr) -> OsString {
    let bytes = arg.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    for &b in bytes {
        out.push(b);
        if b == b'$' {
            out.push(b'$');
        }
    }
    OsString::from_vec(out)
}

/// Checks that `bwrap`, `slirp4netns`, `python3`, and `systemd-run --user` are
/// available before mutating any disk state or database rows.
pub(super) async fn preflight() -> Result<(), AppError> {
    for required in [BWRAP_PATH, SLIRP4NETNS_PATH, PYTHON3_PATH] {
        if !Path::new(required).is_file() {
            return Err(AppError::SupervisorUnavailable(format!(
                "`{required}` is not installed"
            )));
        }
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

async fn ensure_apps_slice() -> Result<(), AppError> {
    let output = user_command("systemctl")
        .args([
            "--user",
            "set-property",
            "--runtime",
            APPS_SLICE,
            &format!("MemoryMax={SLICE_MEMORY_MAX}"),
            &format!("MemorySwapMax={MEMORY_SWAP_MAX}"),
            &format!("TasksMax={SLICE_TASKS_MAX}"),
        ])
        .output()
        .await
        .map_err(|err| {
            AppError::SupervisorUnavailable(format!("cannot configure `{APPS_SLICE}`: {err}"))
        })?;
    if output.status.success() {
        return Ok(());
    }
    let stderr = String::from_utf8_lossy(&output.stderr).trim().to_owned();
    Err(AppError::SupervisorUnavailable(format!(
        "failed to configure `{APPS_SLICE}`: {stderr}"
    )))
}

/// Starts a transient `systemd --user` unit running the sandbox supervisor around `command`.
pub(super) async fn start_unit(spec: &StartUnitSpec) -> Result<(), AppError> {
    let _ = stop_unit(&spec.unit).await;
    ensure_apps_slice().await?;

    let runner_args = write_runtime_artifacts(&SandboxSpec {
        app_dir: spec.app_dir.clone(),
        scratch_dir: spec.scratch_dir.clone(),
        runtime_dir: spec.runtime_dir.clone(),
        log_file: spec.log_file.clone(),
        port: spec.port,
        base_path: spec.base_path.clone(),
        command: spec.command.clone(),
    })?;

    let description = format!("Renoa live app {}", spec.app_name);
    let props = [
        "Type=exec".to_owned(),
        "Restart=on-failure".to_owned(),
        "RestartSec=200ms".to_owned(),
        format!("StartLimitIntervalSec={CRASH_INTERVAL_SECS}s"),
        format!("StartLimitBurst={CRASH_BURST}"),
        format!("MemoryMax={MEMORY_MAX}"),
        format!("MemorySwapMax={MEMORY_SWAP_MAX}"),
        format!("TasksMax={TASKS_MAX}"),
        format!("CPUQuota={CPU_QUOTA}"),
        "TimeoutStopSec=5s".to_owned(),
        "KillMode=control-group".to_owned(),
        "StandardOutput=null".to_owned(),
        "StandardError=journal".to_owned(),
    ];
    let mut args: Vec<OsString> = [
        "--user",
        "--quiet",
        "--slice",
        APPS_SLICE,
        "--unit",
        &spec.unit,
        "--description",
        &description,
    ]
    .into_iter()
    .map(OsString::from)
    .collect();
    for prop in &props {
        args.push(OsString::from("-p"));
        args.push(OsString::from(prop));
    }
    args.push(OsString::from("--"));
    args.extend(runner_args.into_iter().map(|a| escape_systemd_dollars(&a)));

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
            "MainPID",
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
    let mut main_pid: u32 = 0;
    let mut result = "";
    let mut exec_status = "";
    for line in stdout.lines() {
        if let Some((k, v)) = line.split_once('=') {
            match k {
                "LoadState" => load_state = v,
                "ActiveState" => active_state = v,
                "SubState" => sub_state = v,
                "MainPID" => main_pid = v.trim().parse::<u32>().unwrap_or(0),
                "Result" => result = v,
                "ExecMainStatus" => exec_status = v,
                _ => {}
            }
        }
    }

    if load_state == "not-found" || active_state == "inactive" || active_state.is_empty() {
        return Ok(UnitStatus::Inactive);
    }
    if active_state == "failed" || sub_state == "failed" {
        let detail = if result.is_empty() {
            format!("unit failed (exit status {exec_status})")
        } else {
            format!("crash loop / failure ({result}, exit status {exec_status})")
        };
        return Ok(UnitStatus::Failed { detail });
    }
    if active_state == "active" && sub_state == "running" && main_pid > 0 {
        return Ok(UnitStatus::Active { main_pid });
    }
    if matches!(
        active_state,
        "activating" | "reloading" | "deactivating" | "active" | "refreshing"
    ) {
        return Ok(UnitStatus::Activating);
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

fn sync_systemctl() -> std::process::Command {
    let mut cmd = std::process::Command::new("systemctl");
    cmd.stdin(Stdio::null());
    for (k, v) in user_runtime_env() {
        cmd.env(k, v);
    }
    cmd
}

fn stop_specific_unit_sync(unit: &str) -> io::Result<()> {
    let stop_out = sync_systemctl().args(["--user", "stop", unit]).output()?;
    let _ = sync_systemctl()
        .args(["--user", "reset-failed", unit])
        .output();
    if stop_out.status.success() {
        return Ok(());
    }
    let stderr = String::from_utf8_lossy(&stop_out.stderr);
    let lower = stderr.to_ascii_lowercase();
    if lower.contains("not loaded") || lower.contains("no such") {
        return Ok(());
    }
    Err(io::Error::other(format!(
        "systemctl stop `{unit}` failed: {}",
        stderr.trim()
    )))
}

/// Synchronously stops any `renoa-app-<hash12>-*.service` units for `database`
/// during host reset.
///
/// # Errors
/// Returns an `io::Error` if querying or stopping the user units fails.
pub(crate) fn stop_units_sync(database: &Path) -> io::Result<()> {
    let pattern = format!("{}*.service", unit_prefix_for(database));
    let output = sync_systemctl()
        .args([
            "--user",
            "list-units",
            "--all",
            "--plain",
            "--no-legend",
            &pattern,
        ])
        .output()?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(io::Error::other(format!(
            "systemctl list-units failed: {}",
            stderr.trim()
        )));
    }
    let stdout = String::from_utf8_lossy(&output.stdout);
    let mut first_err: Option<io::Error> = None;
    for line in stdout.lines() {
        let Some(unit) = line.split_whitespace().next() else {
            continue;
        };
        if let Err(err) = stop_specific_unit_sync(unit)
            && first_err.is_none()
        {
            first_err = Some(err);
        }
    }
    if let Some(err) = first_err {
        return Err(err);
    }
    Ok(())
}

#[cfg(test)]
pub(crate) fn stop_unit_sync(database: &Path, app_name: &str) {
    let unit = unit_name(database, app_name);
    let _ = stop_specific_unit_sync(&unit);
}

/// RAII drop guard stopping a transient unit if startup is cancelled or fails
/// before the database row commits.
pub(super) struct UnitCleanup {
    unit: String,
    armed: bool,
}

impl UnitCleanup {
    pub(super) fn armed(database: &Path, app_name: &str) -> Self {
        Self::armed_unit(unit_name(database, app_name))
    }

    pub(super) const fn armed_unit(unit: String) -> Self {
        Self { unit, armed: true }
    }

    pub(super) const fn disarm(&mut self) {
        self.armed = false;
    }

    pub(super) async fn cleanup(&mut self) {
        if self.armed {
            self.armed = false;
            let _ = stop_unit(&self.unit).await;
        }
    }
}

impl Drop for UnitCleanup {
    fn drop(&mut self) {
        if self.armed {
            self.armed = false;
            let unit = std::mem::take(&mut self.unit);
            let _ = std::thread::Builder::new()
                .name("renoa-unit-cleanup".into())
                .spawn(move || {
                    let _ = stop_specific_unit_sync(&unit);
                });
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
