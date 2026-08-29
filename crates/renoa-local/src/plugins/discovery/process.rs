use std::{io, path::Path, time::Duration};

use serde::Serialize;
use tokio::{
    io::{AsyncRead, AsyncReadExt as _, AsyncWriteExt as _},
    process::Command,
    task::JoinHandle,
};
use tokio_util::sync::CancellationToken;

use super::{RegistryError, WIRE_VERSION, contract::AdapterRecord};
use crate::process::{child_pid_raw, configure_process_group, stop_process_group_raw};

const PROCESS_DEADLINE: Duration = Duration::from_secs(35);
const MAX_STDOUT_BYTES: usize = 512 * 1_024;
const MAX_STDERR_BYTES: usize = 64 * 1_024;

pub(super) async fn run(
    adapter: &Path,
    request: &impl Serialize,
    cancellation: CancellationToken,
) -> Result<AdapterRecord, RegistryError> {
    if cancellation.is_cancelled() {
        return Err(RegistryError::Cancelled);
    }
    let encoded = serde_json::to_vec(request).map_err(RegistryError::Encode)?;
    let mut command = Command::new("node");
    command
        .arg("--dns-result-order=ipv4first")
        .arg(adapter)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());
    configure_process_group(&mut command);
    let mut child = command.spawn().map_err(RegistryError::Start)?;
    let pid = match child_pid_raw(&child) {
        Ok(pid) => pid,
        Err(error) => {
            child
                .kill()
                .await
                .map_err(|cleanup| RegistryError::Cleanup(cleanup.to_string()))?;
            return Err(RegistryError::Cleanup(error.to_string()));
        }
    };
    let pipes = (child.stdin.take(), child.stdout.take(), child.stderr.take());
    let (Some(stdin), Some(stdout), Some(stderr)) = pipes else {
        stop_process_group_raw(&mut child, pid)
            .await
            .map_err(|error| RegistryError::Cleanup(error.to_string()))?;
        return Err(RegistryError::MissingPipe("configured standard-I/O pipe"));
    };
    let stdout = drain_bounded(stdout, MAX_STDOUT_BYTES);
    let stderr = drain_bounded(stderr, MAX_STDERR_BYTES);
    let deadline = tokio::time::Instant::now() + PROCESS_DEADLINE;

    let write = write_request(stdin, &encoded);
    tokio::pin!(write);
    let write_result = tokio::select! {
        result = &mut write => result.map_err(RegistryError::Write),
        () = cancellation.cancelled() => Err(RegistryError::Cancelled),
        () = tokio::time::sleep_until(deadline) => Err(RegistryError::Timeout),
    };
    if let Err(error) = write_result {
        stop_and_discard(&mut child, pid, stdout, stderr).await?;
        return Err(error);
    }

    let status = tokio::select! {
        result = child.wait() => result.map_err(RegistryError::Wait),
        () = cancellation.cancelled() => Err(RegistryError::Cancelled),
        () = tokio::time::sleep_until(deadline) => Err(RegistryError::Timeout),
    };
    let (stdout, stderr) = stop_and_capture(&mut child, pid, stdout, stderr).await?;
    status?;
    parse(stdout, stderr)
}

async fn write_request(mut stdin: tokio::process::ChildStdin, request: &[u8]) -> io::Result<()> {
    stdin.write_all(request).await?;
    stdin.shutdown().await
}

async fn stop_and_capture(
    child: &mut tokio::process::Child,
    pid: u32,
    stdout: JoinHandle<io::Result<Captured>>,
    stderr: JoinHandle<io::Result<Captured>>,
) -> Result<(Captured, Captured), RegistryError> {
    let cleanup = stop_process_group_raw(child, pid)
        .await
        .map_err(|error| RegistryError::Cleanup(error.to_string()));
    let stdout = join(stdout, "stdout").await;
    let stderr = join(stderr, "stderr").await;
    cleanup?;
    Ok((stdout?, stderr?))
}

async fn stop_and_discard(
    child: &mut tokio::process::Child,
    pid: u32,
    stdout: JoinHandle<io::Result<Captured>>,
    stderr: JoinHandle<io::Result<Captured>>,
) -> Result<(), RegistryError> {
    match stop_and_capture(child, pid, stdout, stderr).await {
        Ok((mut stdout, mut stderr)) => {
            stdout.bytes.fill(0);
            stderr.bytes.fill(0);
            Ok(())
        }
        Err(error) => Err(error),
    }
}

fn parse(mut stdout: Captured, mut stderr: Captured) -> Result<AdapterRecord, RegistryError> {
    if stdout.truncated {
        stdout.bytes.fill(0);
        stderr.bytes.fill(0);
        return Err(RegistryError::OutputLimit);
    }
    let value = super::super::json::parse(&stdout.bytes, "MCP Registry adapter output")
        .map_err(|message| protocol_with_stderr(message, &stderr.bytes));
    let record = value.and_then(|value| {
        serde_json::from_value::<AdapterRecord>(value)
            .map_err(|error| protocol_with_stderr(error.to_string(), &stderr.bytes))
    });
    stdout.bytes.fill(0);
    stderr.bytes.fill(0);
    let record = record?;
    if record.wire_version() != WIRE_VERSION {
        return Err(RegistryError::Protocol(format!(
            "MCP Registry adapter wire version is {}, expected {WIRE_VERSION}",
            record.wire_version()
        )));
    }
    Ok(record)
}

fn protocol_with_stderr(message: String, stderr: &[u8]) -> RegistryError {
    let diagnostic = String::from_utf8_lossy(stderr);
    let diagnostic = diagnostic.trim();
    if diagnostic.is_empty() {
        RegistryError::Protocol(message)
    } else {
        RegistryError::Protocol(format!("{message}; stderr: {diagnostic}"))
    }
}

struct Captured {
    bytes: Vec<u8>,
    truncated: bool,
}

fn drain_bounded(
    mut reader: impl AsyncRead + Unpin + Send + 'static,
    limit: usize,
) -> JoinHandle<io::Result<Captured>> {
    tokio::spawn(async move {
        let mut bytes = Vec::new();
        let mut truncated = false;
        let mut buffer = [0_u8; 8_192];
        loop {
            let read = reader.read(&mut buffer).await?;
            if read == 0 {
                break;
            }
            let remaining = limit.saturating_sub(bytes.len());
            let retained = remaining.min(read);
            bytes.extend_from_slice(&buffer[..retained]);
            truncated |= retained < read;
        }
        Ok(Captured { bytes, truncated })
    })
}

async fn join(
    task: JoinHandle<io::Result<Captured>>,
    stream: &str,
) -> Result<Captured, RegistryError> {
    task.await
        .map_err(|error| RegistryError::Reader(format!("{stream} task failed: {error}")))?
        .map_err(|error| RegistryError::Reader(format!("{stream} read failed: {error}")))
}
