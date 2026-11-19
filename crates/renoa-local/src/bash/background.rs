//! Shell commands that run in the background while the agent carries on.
//!
//! A background command gets its own process group and writes its output to
//! a log file in the workspace. The call returns as soon as it starts, so the
//! agent can talk and take steers while it runs, and checks on it with more
//! shell calls. A watcher owns the process: when the command ends it stops
//! whatever the command left in its group and appends one closing line to the
//! log; at the job's timeout it stops the group. When the Host shuts down its
//! runtime, dropping the watcher kills the group without a closing line; a
//! Host that is killed outright leaves the job to its service manager.

use std::{
    fs::{self, OpenOptions},
    io::Write as _,
    path::{Path, PathBuf},
    process::Stdio,
    time::Duration,
};

use renoa_agent::{ContentBlock, ToolError, ToolOutput};
use serde_json::json;
use tokio::process::{Child, Command};
use uuid::Uuid;

use crate::{
    process::{child_pid, configure_process_group, kill_process_group, stop_process_group},
    tool_error::io_error,
};

#[cfg(test)]
#[path = "background_tests.rs"]
mod tests;

/// The workspace folder that keeps background jobs' output.
pub(super) const JOBS_DIRECTORY: &str = ".renoa-jobs";
/// A background job runs for an hour unless the call asks otherwise.
pub(super) const DEFAULT_SECONDS: u64 = 3_600;
/// The longest a background job may run: six hours, after which the Host
/// stops it, so a forgotten job does not run unwatched.
pub(super) const MAX_SECONDS: u64 = 21_600;

/// Starts `command` in the background in `root`, stopped after
/// `timeout_seconds`, and says where its output goes.
pub(super) fn start(
    root: &Path,
    command: &str,
    timeout_seconds: u64,
) -> Result<ToolOutput, ToolError> {
    let job = Uuid::new_v4();
    let relative = PathBuf::from(JOBS_DIRECTORY).join(format!("{job}.log"));
    let log_path = root.join(&relative);
    fs::create_dir_all(root.join(JOBS_DIRECTORY))
        .map_err(|error| io_error("create the background job folder", &error, false))?;
    let log = OpenOptions::new()
        .create_new(true)
        .append(true)
        .open(&log_path)
        .map_err(|error| io_error("create the background job log", &error, false))?;
    let errors = log
        .try_clone()
        .map_err(|error| io_error("share the background job log", &error, false))?;
    let mut process = Command::new("/bin/bash");
    process
        .args(["-lc", command])
        .current_dir(root)
        .stdin(Stdio::null())
        .stdout(log)
        .stderr(errors);
    configure_process_group(&mut process);
    let started = process
        .spawn()
        .map_err(|error| io_error("start shell", &error, false))
        .and_then(|child| child_pid(&child).map(|pid| (child, pid)));
    let (child, pid) = match started {
        Ok(started) => started,
        Err(error) => {
            // Nothing started, so nothing is left behind: the log goes, and
            // the folder too when this call made it and nothing else is in it.
            let _gone = fs::remove_file(&log_path);
            let _kept_or_gone = fs::remove_dir(root.join(JOBS_DIRECTORY));
            return Err(error);
        }
    };
    // Armed before the watcher runs, so a watcher dropped before its first
    // poll still stops the group.
    tokio::spawn(watch(child, Armed(Some(pid)), log_path, timeout_seconds));
    let shown = relative.display();
    Ok(ToolOutput {
        content: vec![ContentBlock::text(format!(
            "Started in the background as process group {pid}. Its output goes to {shown}, \
and once the command ends, its last line says how. It is stopped after {timeout_seconds} \
seconds. Check on it with bash, a minute or two at a time, for example \
`sleep 60; tail -n 20 {shown}`, and stop it with `kill -- -{pid}`."
        ))],
        details: Some(json!({
            "background": true,
            "process_group": pid,
            "log": relative,
            "timeout_seconds": timeout_seconds,
        })),
        is_error: false,
        closing: None,
    })
}

/// Kills the job's process group if the watcher is dropped before it ends.
struct Armed(Option<u32>);

impl Drop for Armed {
    fn drop(&mut self) {
        if let Some(pid) = self.0 {
            kill_process_group(pid);
        }
    }
}

async fn watch(mut child: Child, mut armed: Armed, log_path: PathBuf, timeout_seconds: u64) {
    let Some(pid) = armed.0 else {
        return;
    };
    let ended = tokio::time::timeout(Duration::from_secs(timeout_seconds), child.wait()).await;
    let line = match ended {
        Ok(Ok(status)) => status.code().map_or_else(
            || "[the command ended by a signal]".to_owned(),
            |code| format!("[the command exited with code {code}]"),
        ),
        Ok(Err(error)) => format!("[the Host could not wait for the command: {error}]"),
        Err(_) => format!("[the Host stopped the command after {timeout_seconds} seconds]"),
    };
    // Whatever the command started is stopped with it, as for any command.
    let stopped = stop_process_group(&mut child, pid).await;
    armed.0 = None;
    let line = match stopped {
        Ok(()) => line,
        Err(error) => format!("{line}\n[stopping what it left running failed: {error}]"),
    };
    // The log's folder may be gone with the workspace; there is then no one
    // to tell.
    if let Ok(mut log) = OpenOptions::new().append(true).open(&log_path) {
        let _ = writeln!(log, "\n{line}");
    }
}
