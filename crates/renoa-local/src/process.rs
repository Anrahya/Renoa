use std::{collections::VecDeque, io, process::Stdio, time::Duration};

use nix::{
    sys::signal::{Signal, killpg},
    unistd::Pid,
};
use renoa_agent::ToolError;
use thiserror::Error;
use tokio::{
    io::{AsyncRead, AsyncReadExt},
    process::{Child, Command},
    task::JoinHandle,
};

use crate::output::MAX_TOOL_OUTPUT_BYTES;
use crate::tool_error::io_error;

const PROCESS_GROUP_EXIT_DEADLINE: Duration = Duration::from_secs(5);

pub(crate) fn configure(command: &mut Command) {
    configure_process_group(command);
    command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
}

pub(crate) fn configure_process_group(command: &mut Command) {
    command.kill_on_drop(true);
    std::os::unix::process::CommandExt::process_group(command.as_std_mut(), 0);
}

pub(crate) fn child_pid(child: &Child) -> Result<u32, ToolError> {
    child_pid_raw(child).map_err(|error| ToolError::outcome_unknown(error.to_string()))
}

pub(crate) async fn stop_process_group(child: &mut Child, pid: u32) -> Result<(), ToolError> {
    stop_process_group_raw(child, pid)
        .await
        .map_err(|error| ToolError::outcome_unknown(error.to_string()))
}

pub(crate) fn child_pid_raw(child: &Child) -> Result<u32, ProcessGroupError> {
    child.id().ok_or(ProcessGroupError::MissingPid)
}

pub(crate) async fn stop_process_group_raw(
    child: &mut Child,
    pid: u32,
) -> Result<(), ProcessGroupError> {
    signal_process_group(pid, Signal::SIGTERM)?;
    if let Ok(result) = tokio::time::timeout(Duration::from_millis(500), async {
        child.wait().await.map_err(|source| ProcessGroupError::Io {
            action: "reap cancelled process",
            source,
        })?;
        wait_for_process_group_raw(pid).await
    })
    .await
    {
        return result;
    }
    signal_process_group(pid, Signal::SIGKILL)?;
    child.wait().await.map_err(|source| ProcessGroupError::Io {
        action: "reap killed process",
        source,
    })?;
    wait_for_process_group_raw(pid).await
}

pub(crate) async fn wait_for_process_group(pid: u32) -> Result<(), ToolError> {
    wait_for_process_group_raw(pid)
        .await
        .map_err(|error| ToolError::outcome_unknown(error.to_string()))
}

pub(crate) async fn wait_for_process_group_raw(pid: u32) -> Result<(), ProcessGroupError> {
    let pid = process_group_id(pid)?;
    let deadline = tokio::time::Instant::now() + PROCESS_GROUP_EXIT_DEADLINE;
    loop {
        match killpg(pid, None) {
            Err(nix::errno::Errno::ESRCH) => return Ok(()),
            Ok(()) if tokio::time::Instant::now() < deadline => {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
            Ok(()) => return Err(ProcessGroupError::ExitTimeout),
            Err(source) => return Err(ProcessGroupError::Inspect(source)),
        }
    }
}

fn signal_process_group(pid: u32, signal: Signal) -> Result<(), ProcessGroupError> {
    match killpg(process_group_id(pid)?, signal) {
        Ok(()) | Err(nix::errno::Errno::ESRCH) => Ok(()),
        Err(source) => Err(ProcessGroupError::Signal(source)),
    }
}

fn process_group_id(pid: u32) -> Result<Pid, ProcessGroupError> {
    i32::try_from(pid)
        .map(Pid::from_raw)
        .map_err(|_| ProcessGroupError::InvalidPid)
}

#[derive(Debug, Error)]
pub(crate) enum ProcessGroupError {
    #[error("spawned process has no process id")]
    MissingPid,
    #[error("process id is out of range")]
    InvalidPid,
    #[error("cannot signal process group: {0}")]
    Signal(nix::errno::Errno),
    #[error("cannot inspect process group: {0}")]
    Inspect(nix::errno::Errno),
    #[error("process group remained alive after its 5-second cleanup deadline")]
    ExitTimeout,
    #[error("cannot {action}: {source}")]
    Io {
        action: &'static str,
        #[source]
        source: io::Error,
    },
}

pub(crate) struct CapturedTail {
    pub(crate) bytes: Vec<u8>,
    pub(crate) total_bytes: usize,
}

impl CapturedTail {
    pub(crate) fn truncated(&self) -> bool {
        self.total_bytes > self.bytes.len()
    }
}

pub(crate) fn drain_tail(
    mut reader: impl AsyncRead + Unpin + Send + 'static,
) -> JoinHandle<io::Result<CapturedTail>> {
    tokio::spawn(async move {
        let mut retained = VecDeque::with_capacity(MAX_TOOL_OUTPUT_BYTES);
        let mut total_bytes = 0_usize;
        let mut buffer = [0_u8; 8_192];
        loop {
            let read = reader.read(&mut buffer).await?;
            if read == 0 {
                break;
            }
            total_bytes = total_bytes.saturating_add(read);
            retain_tail(&mut retained, &buffer[..read]);
        }
        Ok(CapturedTail {
            bytes: retained.into_iter().collect(),
            total_bytes,
        })
    })
}

fn retain_tail(retained: &mut VecDeque<u8>, bytes: &[u8]) {
    if bytes.len() >= MAX_TOOL_OUTPUT_BYTES {
        retained.clear();
        retained.extend(&bytes[bytes.len() - MAX_TOOL_OUTPUT_BYTES..]);
        return;
    }
    let excess = retained
        .len()
        .saturating_add(bytes.len())
        .saturating_sub(MAX_TOOL_OUTPUT_BYTES);
    retained.drain(..excess);
    retained.extend(bytes);
}

pub(crate) async fn join_tail(
    output: JoinHandle<io::Result<CapturedTail>>,
) -> Result<CapturedTail, ToolError> {
    output
        .await
        .map_err(|error| ToolError::io(format!("output reader failed: {error}"), false))?
        .map_err(|error| io_error("read process output", &error, false))
}
