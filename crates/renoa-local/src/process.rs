use std::{collections::VecDeque, io, process::Stdio, time::Duration};

use nix::{
    sys::signal::{Signal, killpg},
    unistd::Pid,
};
use renoa_agent::ToolError;
use tokio::{
    io::{AsyncRead, AsyncReadExt},
    process::{Child, Command},
    task::JoinHandle,
};

use crate::output::MAX_TOOL_OUTPUT_BYTES;

pub(crate) fn configure(command: &mut Command) {
    command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    std::os::unix::process::CommandExt::process_group(command.as_std_mut(), 0);
}

pub(crate) fn child_pid(child: &Child) -> Result<u32, ToolError> {
    child
        .id()
        .ok_or_else(|| ToolError::new("spawned process has no process id"))
}

pub(crate) async fn stop_process_group(child: &mut Child, pid: u32) -> Result<(), ToolError> {
    signal_process_group(pid, Signal::SIGTERM)?;
    if let Ok(result) = tokio::time::timeout(Duration::from_millis(500), async {
        child
            .wait()
            .await
            .map_err(|error| tool_error("reap cancelled process", error))?;
        wait_for_process_group(pid).await
    })
    .await
    {
        return result;
    }
    signal_process_group(pid, Signal::SIGKILL)?;
    child
        .wait()
        .await
        .map_err(|error| tool_error("reap killed process", error))?;
    wait_for_process_group(pid).await
}

pub(crate) async fn wait_for_process_group(pid: u32) -> Result<(), ToolError> {
    let pid = process_group_id(pid)?;
    loop {
        match killpg(pid, None) {
            Err(nix::errno::Errno::ESRCH) => return Ok(()),
            Ok(()) => tokio::time::sleep(Duration::from_millis(10)).await,
            Err(error) => {
                return Err(ToolError::new(format!(
                    "cannot inspect process group: {error}"
                )));
            }
        }
    }
}

fn signal_process_group(pid: u32, signal: Signal) -> Result<(), ToolError> {
    match killpg(process_group_id(pid)?, signal) {
        Ok(()) | Err(nix::errno::Errno::ESRCH) => Ok(()),
        Err(error) => Err(ToolError::new(format!(
            "cannot signal process group: {error}"
        ))),
    }
}

fn process_group_id(pid: u32) -> Result<Pid, ToolError> {
    i32::try_from(pid)
        .map(Pid::from_raw)
        .map_err(|_| ToolError::new("process id is out of range"))
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
        .map_err(|error| ToolError::new(format!("output reader failed: {error}")))?
        .map_err(|error| tool_error("read process output", error))
}

fn tool_error(action: &str, error: impl std::fmt::Display) -> ToolError {
    ToolError::new(format!("cannot {action}: {error}"))
}
