//! Bubblewrap + slirp4netns sandbox runtime artifacts and bounded log tailing.

use std::{
    ffi::OsString,
    fs, io,
    os::unix::fs::PermissionsExt as _,
    path::{Path, PathBuf},
};

use serde::Deserialize;

use super::AppError;
use super::model::{
    LOG_TAIL_BYTES, LOG_TAIL_LINES, MAX_LOG_FILE_BYTES, MAX_SCRATCH_BYTES, STARTUP_TIMEOUT,
};

pub(crate) const BWRAP_PATH: &str = "/usr/bin/bwrap";
pub(crate) const SLIRP4NETNS_PATH: &str = "/usr/bin/slirp4netns";
pub(crate) const PYTHON3_PATH: &str = "/usr/bin/python3";
pub(crate) const SANDBOX_STATE_NAME: &str = "sandbox.json";

const COMMAND_SCRIPT_NAME: &str = "command.sh";
const CONFIG_FILE_NAME: &str = "config.json";
const RUNNER_SCRIPT_NAME: &str = "runner.py";
const RESOLV_CONF_NAME: &str = "resolv.conf";
const HOSTS_FILE_NAME: &str = "hosts";
const SANDBOX_PATH: &str = "/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin";
const RUNNER_PY: &str = include_str!("runner.py");

#[derive(Debug, Clone)]
pub(crate) struct SandboxSpec {
    pub app_dir: PathBuf,
    pub scratch_dir: PathBuf,
    pub runtime_dir: PathBuf,
    pub log_file: PathBuf,
    pub port: u16,
    pub base_path: String,
    pub command: String,
}

#[derive(Debug, Deserialize)]
pub(crate) struct SandboxState {
    pub child_pid: u32,
    pub port: u16,
}

pub(crate) fn read_sandbox_state(runtime_dir: &Path) -> Result<Option<SandboxState>, String> {
    let path = runtime_dir.join(SANDBOX_STATE_NAME);
    let raw = match fs::read_to_string(&path) {
        Ok(raw) => raw,
        Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(err) => return Err(format!("cannot read `{}`: {err}", path.display())),
    };
    serde_json::from_str::<SandboxState>(&raw)
        .map(Some)
        .map_err(|err| format!("invalid `{}`: {err}", path.display()))
}

pub(crate) fn check_scratch_quota(scratch_dir: &Path) -> Result<(), AppError> {
    if !scratch_dir.exists() {
        return Ok(());
    }
    let mut stack = vec![scratch_dir.to_path_buf()];
    let mut total: u64 = 0;

    while let Some(dir) = stack.pop() {
        let meta = fs::symlink_metadata(&dir)?;
        if meta.file_type().is_symlink() || !meta.is_dir() {
            return Err(AppError::StartupFailed {
                reason: format!("scratch path `{}` is not a real directory", dir.display()),
            });
        }
        for entry in fs::read_dir(&dir)? {
            let entry = entry?;
            let path = entry.path();
            let entry_meta = fs::symlink_metadata(&path)?;
            if entry_meta.file_type().is_symlink() {
                continue;
            }
            if entry_meta.is_dir() {
                stack.push(path);
            } else if entry_meta.is_file() {
                total = total.saturating_add(entry_meta.len());
                if total > MAX_SCRATCH_BYTES {
                    return Err(AppError::StartupFailed {
                        reason: format!("scratch directory exceeds {MAX_SCRATCH_BYTES} byte limit"),
                    });
                }
            }
        }
    }
    Ok(())
}

pub(crate) fn write_runtime_artifacts(spec: &SandboxSpec) -> Result<Vec<OsString>, AppError> {
    check_scratch_quota(&spec.scratch_dir)?;

    let cmd_path = spec.runtime_dir.join(COMMAND_SCRIPT_NAME);
    let cmd_content = format!("#!/bin/sh\n{}\n", spec.command);
    write_private_file(&cmd_path, cmd_content.as_bytes(), 0o700)?;

    let resolv_path = spec.runtime_dir.join(RESOLV_CONF_NAME);
    write_private_file(&resolv_path, b"nameserver 10.0.2.3\n", 0o600)?;

    let hosts_path = spec.runtime_dir.join(HOSTS_FILE_NAME);
    write_private_file(&hosts_path, b"127.0.0.1 localhost\n", 0o600)?;

    let runner_path = spec.runtime_dir.join(RUNNER_SCRIPT_NAME);
    write_private_file(&runner_path, RUNNER_PY.as_bytes(), 0o700)?;

    let config_path = spec.runtime_dir.join(CONFIG_FILE_NAME);
    let config_json = serde_json::json!({
        "app_dir": spec.app_dir,
        "scratch_dir": spec.scratch_dir,
        "log_file": spec.log_file,
        "sandbox_state": spec.runtime_dir.join(SANDBOX_STATE_NAME),
        "command_script": cmd_path,
        "resolv_conf": resolv_path,
        "hosts_file": hosts_path,
        "bwrap_path": BWRAP_PATH,
        "slirp4netns_path": SLIRP4NETNS_PATH,
        "port": spec.port,
        "base_path": spec.base_path,
        "path_env": SANDBOX_PATH,
        "max_log_bytes": MAX_LOG_FILE_BYTES,
        "startup_timeout_secs": STARTUP_TIMEOUT.as_secs_f64(),
    });
    let serialized = serde_json::to_vec_pretty(&config_json).map_err(|err| {
        AppError::Io(io::Error::other(format!("serialize sandbox config: {err}")))
    })?;
    write_private_file(&config_path, &serialized, 0o600)?;

    Ok(vec![
        OsString::from(PYTHON3_PATH),
        OsString::from("-I"),
        runner_path.into_os_string(),
        config_path.into_os_string(),
    ])
}

pub(crate) fn read_log_tail(log_file: &Path) -> String {
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

fn write_private_file(path: &Path, contents: &[u8], mode: u32) -> io::Result<()> {
    let tmp = path.with_extension("tmp");
    fs::write(&tmp, contents)?;
    fs::set_permissions(&tmp, fs::Permissions::from_mode(mode))?;
    fs::rename(&tmp, path)
}
