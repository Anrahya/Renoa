use std::{
    env,
    path::{Path, PathBuf},
    process::ExitStatus,
};

use renoa_agent::ToolError;
use tokio::{
    process::{Child, ChildStdout, Command},
    task::JoinHandle,
};
use tokio_util::sync::CancellationToken;

use crate::{
    process::{
        CapturedTail, child_pid, configure, drain_tail, join_tail, stop_process_group,
        wait_for_process_group,
    },
    tool_error::io_error,
    workspace::{LocalWorkspaceError, hex_sha256},
};

pub(crate) struct Ripgrep {
    executable: PathBuf,
    revision: String,
}

impl Ripgrep {
    pub(crate) fn discover() -> Result<Self, LocalWorkspaceError> {
        let path = env::var_os("PATH").ok_or(LocalWorkspaceError::RipgrepUnavailable)?;
        let executable = env::split_paths(&path)
            .map(|directory| directory.join("rg"))
            .find(|candidate| candidate.is_file())
            .ok_or(LocalWorkspaceError::RipgrepUnavailable)?;
        let executable =
            std::fs::canonicalize(executable).map_err(LocalWorkspaceError::RipgrepInspection)?;
        let output = std::process::Command::new(&executable)
            .arg("--version")
            .env_remove("RIPGREP_CONFIG_PATH")
            .output()
            .map_err(LocalWorkspaceError::RipgrepInspection)?;
        let version = String::from_utf8(output.stdout.clone())
            .map_err(|_| LocalWorkspaceError::InvalidRipgrepVersion)?;
        if !output.status.success() || !version.starts_with("ripgrep ") {
            return Err(LocalWorkspaceError::InvalidRipgrepVersion);
        }
        Ok(Self {
            executable,
            revision: hex_sha256(&output.stdout),
        })
    }

    pub(crate) fn revision(&self) -> &str {
        &self.revision
    }

    pub(crate) fn command(&self, root: &Path) -> Command {
        let mut command = Command::new(&self.executable);
        command
            .current_dir(root)
            .env_remove("RIPGREP_CONFIG_PATH")
            .args([
                "--no-config",
                "--color=never",
                "--glob=!.git/",
                "--sort=path",
            ]);
        command
    }
}

pub(crate) struct SearchProcess {
    child: Child,
    pid: u32,
    stderr_task: Option<JoinHandle<std::io::Result<CapturedTail>>>,
    stderr: CapturedTail,
}

impl SearchProcess {
    pub(crate) fn start(mut command: Command, name: &str) -> Result<Self, ToolError> {
        configure(&mut command);
        let mut child = command
            .spawn()
            .map_err(|error| io_error(&format!("start {name}"), &error, false))?;
        let pid = child_pid(&child)?;
        let stderr = child
            .stderr
            .take()
            .ok_or_else(|| ToolError::outcome_unknown(format!("{name} stderr was not piped")))?;
        Ok(Self {
            child,
            pid,
            stderr_task: Some(drain_tail(stderr)),
            stderr: CapturedTail {
                bytes: Vec::new(),
                total_bytes: 0,
            },
        })
    }

    pub(crate) fn take_stdout(&mut self, name: &str) -> Result<ChildStdout, ToolError> {
        self.child
            .stdout
            .take()
            .ok_or_else(|| ToolError::outcome_unknown(format!("{name} stdout was not piped")))
    }

    pub(crate) async fn stop(&mut self) -> Result<(), ToolError> {
        stop_process_group(&mut self.child, self.pid).await?;
        self.collect_stderr().await
    }

    pub(crate) async fn finish(
        &mut self,
        cancellation: &CancellationToken,
        name: &str,
    ) -> Result<ExitStatus, ToolError> {
        let status = tokio::select! {
            biased;
            () = cancellation.cancelled() => {
                self.stop().await?;
                return Err(ToolError::cancelled(
                    format!("{name} execution was cancelled"),
                    false,
                ));
            }
            status = self.child.wait() => {
                status.map_err(|error| ToolError::outcome_unknown(format!("cannot wait for {name}: {error}")))?
            }
        };
        let (group, stderr) = tokio::join!(wait_for_process_group(self.pid), self.take_stderr());
        group?;
        self.stderr = stderr?;
        Ok(status)
    }

    pub(crate) fn validate_status(
        &self,
        status: ExitStatus,
        no_result_is_one: bool,
    ) -> Result<(), ToolError> {
        if status.success() || (no_result_is_one && status.code() == Some(1)) {
            return Ok(());
        }
        let diagnostic = String::from_utf8_lossy(&self.stderr.bytes);
        Err(ToolError::process_failed(
            format!(
                "ripgrep exited with code {}: {}",
                status
                    .code()
                    .map_or_else(|| "unknown".to_owned(), |code| code.to_string()),
                diagnostic.trim()
            ),
            false,
        ))
    }

    async fn collect_stderr(&mut self) -> Result<(), ToolError> {
        self.stderr = self.take_stderr().await?;
        Ok(())
    }

    async fn take_stderr(&mut self) -> Result<CapturedTail, ToolError> {
        let task = self
            .stderr_task
            .take()
            .ok_or_else(|| ToolError::internal("ripgrep stderr was already collected"))?;
        join_tail(task).await
    }
}
