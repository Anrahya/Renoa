use std::{io, path::Path, time::Duration};

use serde::{Deserialize, Serialize};
use tokio::{
    io::{AsyncRead, AsyncReadExt as _, AsyncWriteExt as _},
    process::Command,
    sync::oneshot,
    task::JoinHandle,
};
use tokio_util::sync::CancellationToken;

use super::{
    AdapterCatalog, McpAdapterError, McpCatalogSnapshot, McpCredentialHeader, McpHostError,
    McpRemoteFailure, McpRequestHeaders,
};
use crate::process::{child_pid_raw, configure_process_group, stop_process_group_raw};

#[cfg(test)]
mod tests;

const WIRE_VERSION: u32 = 8;
const PROCESS_DEADLINE: Duration = Duration::from_secs(35);
const MAX_STDOUT_BYTES: usize = 20 * 1_024 * 1_024;
const MAX_STDERR_BYTES: usize = 64 * 1_024;

pub(crate) async fn discover(
    adapter: &Path,
    connection_id: &str,
    endpoint: &str,
    request_headers: &McpRequestHeaders,
    credential: Option<&McpCredentialHeader>,
) -> Result<McpCatalogSnapshot, McpHostError> {
    discover_cancellable(
        adapter,
        connection_id,
        endpoint,
        request_headers,
        credential,
        CancellationToken::new(),
    )
    .await
}

pub(crate) async fn discover_cancellable(
    adapter: &Path,
    connection_id: &str,
    endpoint: &str,
    request_headers: &McpRequestHeaders,
    credential: Option<&McpCredentialHeader>,
    cancellation: CancellationToken,
) -> Result<McpCatalogSnapshot, McpHostError> {
    if cancellation.is_cancelled() {
        return Err(McpAdapterError::Cancelled.into());
    }
    let mut request = serde_json::to_vec(&DiscoverRequest {
        wire_version: WIRE_VERSION,
        action: "discover",
        endpoint,
        headers: request_headers,
        credential: credential.map(WireCredential::from),
    })
    .map_err(McpAdapterError::Encode)?;
    let result = run_adapter(adapter, &request, credential, cancellation).await;
    request.fill(0);
    let catalog = result?;
    McpCatalogSnapshot::from_adapter_with_headers(connection_id, request_headers.clone(), catalog)
}

#[derive(Serialize)]
struct DiscoverRequest<'a> {
    wire_version: u32,
    action: &'static str,
    endpoint: &'a str,
    #[serde(skip_serializing_if = "McpRequestHeaders::is_empty")]
    headers: &'a McpRequestHeaders,
    #[serde(skip_serializing_if = "Option::is_none")]
    credential: Option<WireCredential<'a>>,
}

#[derive(Serialize)]
struct WireCredential<'a> {
    scheme: &'static str,
    name: &'a str,
    prefix: &'a str,
    secret: &'a str,
}

impl<'a> From<&'a McpCredentialHeader> for WireCredential<'a> {
    fn from(credential: &'a McpCredentialHeader) -> Self {
        Self {
            scheme: "header",
            name: credential.name(),
            prefix: credential.prefix(),
            secret: credential.secret(),
        }
    }
}

async fn run_adapter(
    adapter: &Path,
    request: &[u8],
    credential: Option<&McpCredentialHeader>,
    cancellation: CancellationToken,
) -> Result<AdapterCatalog, McpAdapterError> {
    let deadline = tokio::time::Instant::now() + PROCESS_DEADLINE;
    let mut command = Command::new("node");
    command
        .arg("--dns-result-order=ipv4first")
        .arg(adapter)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());
    configure_process_group(&mut command);
    let mut child = command.spawn().map_err(McpAdapterError::Start)?;
    let pid = match child_pid_raw(&child) {
        Ok(pid) => pid,
        Err(error) => {
            child
                .kill()
                .await
                .map_err(|cleanup| McpAdapterError::Cleanup(cleanup.to_string()))?;
            return Err(McpAdapterError::Cleanup(error.to_string()));
        }
    };
    let pipes = (child.stdin.take(), child.stdout.take(), child.stderr.take());
    let (Some(mut stdin), Some(stdout), Some(stderr)) = pipes else {
        stop_process_group_raw(&mut child, pid)
            .await
            .map_err(|error| McpAdapterError::Cleanup(error.to_string()))?;
        return Err(McpAdapterError::MissingPipe("configured standard-I/O pipe"));
    };
    let (terminal_sender, mut terminal_receiver) = oneshot::channel();
    let stdout = drain_head(stdout, MAX_STDOUT_BYTES, Some(terminal_sender));
    let stderr = drain_head(stderr, MAX_STDERR_BYTES, None);

    let write_result = write_before_deadline(&mut stdin, request, deadline, &cancellation).await;
    drop(stdin);
    match write_result {
        WriteSignal::Complete(Ok(())) => {}
        WriteSignal::Complete(Err(source)) => {
            stop_and_discard(&mut child, pid, stdout, stderr).await?;
            return Err(McpAdapterError::Write(source));
        }
        WriteSignal::Deadline => {
            stop_and_discard(&mut child, pid, stdout, stderr).await?;
            return Err(McpAdapterError::Timeout);
        }
        WriteSignal::Cancelled => {
            stop_and_discard(&mut child, pid, stdout, stderr).await?;
            return Err(McpAdapterError::Cancelled);
        }
    }

    let signal = wait_for_signal(&mut child, &mut terminal_receiver, deadline, &cancellation).await;
    match signal {
        ProcessSignal::Exited(Err(source)) => {
            stop_and_capture(&mut child, pid, stdout, stderr).await?;
            Err(McpAdapterError::Wait(source))
        }
        ProcessSignal::Exited(Ok(status)) => {
            let (stdout, stderr) = stop_and_capture(&mut child, pid, stdout, stderr).await?;
            parse_captured(stdout, stderr, &format!("{status}"), credential)
        }
        ProcessSignal::Terminal => {
            let (stdout, stderr) = stop_and_capture(&mut child, pid, stdout, stderr).await?;
            parse_captured(stdout, stderr, "stopped after terminal record", credential)
        }
        ProcessSignal::Deadline => {
            let (mut stdout, mut stderr) =
                stop_and_capture(&mut child, pid, stdout, stderr).await?;
            if stdout.bytes.contains(&b'\n') {
                parse_captured(stdout, stderr, "stopped at Host deadline", credential)
            } else {
                stdout.bytes.fill(0);
                stderr.bytes.fill(0);
                Err(McpAdapterError::Timeout)
            }
        }
        ProcessSignal::Cancelled => {
            stop_and_discard(&mut child, pid, stdout, stderr).await?;
            Err(McpAdapterError::Cancelled)
        }
    }
}

async fn wait_for_signal(
    child: &mut tokio::process::Child,
    terminal: &mut oneshot::Receiver<()>,
    deadline: tokio::time::Instant,
    cancellation: &CancellationToken,
) -> ProcessSignal {
    let wait = child.wait();
    tokio::pin!(wait);
    tokio::select! {
        biased;
        terminal = terminal => if terminal.is_ok() {
            ProcessSignal::Terminal
        } else {
            tokio::select! {
                biased;
                status = &mut wait => ProcessSignal::Exited(status),
                () = cancellation.cancelled() => ProcessSignal::Cancelled,
                () = tokio::time::sleep_until(deadline) => ProcessSignal::Deadline,
            }
        },
        status = &mut wait => ProcessSignal::Exited(status),
        () = cancellation.cancelled() => ProcessSignal::Cancelled,
        () = tokio::time::sleep_until(deadline) => ProcessSignal::Deadline,
    }
}

async fn write_before_deadline(
    stdin: &mut tokio::process::ChildStdin,
    request: &[u8],
    deadline: tokio::time::Instant,
    cancellation: &CancellationToken,
) -> WriteSignal {
    let write = async {
        stdin.write_all(request).await?;
        stdin.shutdown().await
    };
    tokio::pin!(write);
    tokio::select! {
        biased;
        result = &mut write => WriteSignal::Complete(result),
        () = cancellation.cancelled() => WriteSignal::Cancelled,
        () = tokio::time::sleep_until(deadline) => WriteSignal::Deadline,
    }
}

enum WriteSignal {
    Complete(io::Result<()>),
    Deadline,
    Cancelled,
}

fn parse_captured(
    mut stdout: CapturedHead,
    mut stderr: CapturedHead,
    status: &str,
    credential: Option<&McpCredentialHeader>,
) -> Result<AdapterCatalog, McpAdapterError> {
    if stdout.truncated {
        stdout.bytes.fill(0);
        stderr.bytes.fill(0);
        return Err(McpAdapterError::OutputLimit);
    }
    let terminal = parse_discovery_record(&stdout.bytes);
    let result = match terminal {
        Ok(Ok(mut catalog)) => {
            catalog.redact_credential(credential);
            Ok(catalog)
        }
        Ok(Err(McpAdapterError::Remote(mut failure))) => {
            failure.redact_credential(credential);
            Err(McpAdapterError::Remote(failure))
        }
        Ok(Err(error)) => Err(error),
        Err(protocol) => {
            let mut diagnostic = String::from_utf8_lossy(&stderr.bytes).into_owned();
            if let Some(credential) = credential {
                credential.redact_text(&mut diagnostic);
            }
            let suffix = if diagnostic.trim().is_empty() {
                String::new()
            } else {
                format!("; stderr: {}", diagnostic.trim())
            };
            Err(McpAdapterError::Protocol(format!(
                "{protocol}{suffix}; process status {status}"
            )))
        }
    };
    stdout.bytes.fill(0);
    stderr.bytes.fill(0);
    result
}

async fn stop_and_capture(
    child: &mut tokio::process::Child,
    pid: u32,
    stdout: JoinHandle<io::Result<CapturedHead>>,
    stderr: JoinHandle<io::Result<CapturedHead>>,
) -> Result<(CapturedHead, CapturedHead), McpAdapterError> {
    let cleanup = stop_process_group_raw(child, pid)
        .await
        .map_err(|error| McpAdapterError::Cleanup(error.to_string()));
    let stdout = join_capture(stdout, "stdout").await;
    let stderr = join_capture(stderr, "stderr").await;
    cleanup?;
    Ok((stdout?, stderr?))
}

async fn stop_and_discard(
    child: &mut tokio::process::Child,
    pid: u32,
    stdout: JoinHandle<io::Result<CapturedHead>>,
    stderr: JoinHandle<io::Result<CapturedHead>>,
) -> Result<(), McpAdapterError> {
    let (mut stdout, mut stderr) = stop_and_capture(child, pid, stdout, stderr).await?;
    stdout.bytes.fill(0);
    stderr.bytes.fill(0);
    Ok(())
}

enum ProcessSignal {
    Exited(io::Result<std::process::ExitStatus>),
    Terminal,
    Deadline,
    Cancelled,
}

fn parse_discovery_record(
    encoded: &[u8],
) -> Result<Result<AdapterCatalog, McpAdapterError>, String> {
    let mut records = encoded
        .split(|byte| *byte == b'\n')
        .filter(|line| !line.is_empty());
    let record = records
        .next()
        .ok_or_else(|| "adapter returned no terminal record".to_owned())?;
    if records.next().is_some() {
        return Err("adapter returned more than one discovery record".to_owned());
    }
    let header: RecordHeader =
        serde_json::from_slice(record).map_err(|error| format!("decode record header: {error}"))?;
    if header.wire_version != WIRE_VERSION {
        return Err(format!(
            "adapter wire version {} is unsupported; expected {WIRE_VERSION}",
            header.wire_version
        ));
    }
    match header.event.as_str() {
        "discovered" => serde_json::from_slice::<DiscoveredRecord>(record)
            .map(|record| Ok(record.catalog))
            .map_err(|error| format!("decode discovered record: {error}")),
        "failed" => serde_json::from_slice::<FailedRecord>(record)
            .map_err(|error| format!("decode failed record: {error}"))
            .and_then(|record| {
                record
                    .failure
                    .validate_wire()
                    .map_err(|error| format!("invalid failed record: {error}"))?;
                Ok(Err(McpAdapterError::Remote(record.failure)))
            }),
        event => Err(format!(
            "adapter returned unexpected '{event}' record for discovery"
        )),
    }
}

#[derive(Deserialize)]
struct RecordHeader {
    wire_version: u32,
    event: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct DiscoveredRecord {
    #[serde(rename = "wire_version")]
    _wire_version: u32,
    #[serde(rename = "event")]
    _event: DiscoveredEvent,
    catalog: AdapterCatalog,
}

#[derive(Deserialize)]
enum DiscoveredEvent {
    #[serde(rename = "discovered")]
    Discovered,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct FailedRecord {
    #[serde(rename = "wire_version")]
    _wire_version: u32,
    #[serde(rename = "event")]
    _event: FailedEvent,
    failure: McpRemoteFailure,
}

#[derive(Deserialize)]
enum FailedEvent {
    #[serde(rename = "failed")]
    Failed,
}

struct CapturedHead {
    bytes: Vec<u8>,
    truncated: bool,
}

impl Drop for CapturedHead {
    fn drop(&mut self) {
        self.bytes.fill(0);
    }
}

fn drain_head(
    mut reader: impl AsyncRead + Unpin + Send + 'static,
    limit: usize,
    mut first_record: Option<oneshot::Sender<()>>,
) -> JoinHandle<io::Result<CapturedHead>> {
    tokio::spawn(async move {
        let mut bytes = Vec::new();
        let mut buffer = [0_u8; 8_192];
        let mut truncated = false;
        loop {
            let read = reader.read(&mut buffer).await?;
            if read == 0 {
                break;
            }
            let previous_length = bytes.len();
            let retained = read.min(limit.saturating_sub(previous_length));
            bytes.extend_from_slice(&buffer[..retained]);
            truncated |= retained < read;
            if !truncated
                && bytes[previous_length..].contains(&b'\n')
                && let Some(sender) = first_record.take()
            {
                let _receiver_closed = sender.send(()).is_err();
            }
        }
        Ok(CapturedHead { bytes, truncated })
    })
}

async fn join_capture(
    task: JoinHandle<io::Result<CapturedHead>>,
    stream: &'static str,
) -> Result<CapturedHead, McpAdapterError> {
    task.await
        .map_err(|source| McpAdapterError::ReaderTask(stream, source))?
        .map_err(|source| McpAdapterError::Read { stream, source })
}
