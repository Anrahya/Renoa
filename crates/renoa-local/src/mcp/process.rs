use std::{io, path::Path, time::Duration};

use serde::{Deserialize, Serialize};
use tokio::{
    io::{AsyncRead, AsyncReadExt as _, AsyncWriteExt as _},
    process::Command,
    sync::oneshot,
    task::JoinHandle,
};

use super::{AdapterCatalog, McpAdapterError, McpCatalogSnapshot, McpHostError, McpRemoteFailure};
use crate::process::{child_pid_raw, configure_process_group, stop_process_group_raw};

const WIRE_VERSION: u32 = 1;
const PROCESS_DEADLINE: Duration = Duration::from_secs(35);
const MAX_STDOUT_BYTES: usize = 20 * 1_024 * 1_024;
const MAX_STDERR_BYTES: usize = 64 * 1_024;

pub(crate) async fn discover(
    adapter: &Path,
    connection_id: &str,
    endpoint: &str,
) -> Result<McpCatalogSnapshot, McpHostError> {
    let request = serde_json::to_vec(&DiscoverRequest {
        wire_version: WIRE_VERSION,
        action: "discover",
        endpoint,
    })
    .map_err(McpAdapterError::Encode)?;
    let catalog = run_adapter(adapter, &request).await?;
    McpCatalogSnapshot::from_adapter(connection_id, catalog)
}

#[derive(Serialize)]
struct DiscoverRequest<'a> {
    wire_version: u32,
    action: &'static str,
    endpoint: &'a str,
}

async fn run_adapter(adapter: &Path, request: &[u8]) -> Result<AdapterCatalog, McpAdapterError> {
    let mut command = Command::new("node");
    command
        .arg("--dns-result-order=ipv4first")
        .arg(adapter)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());
    configure_process_group(&mut command);
    let mut child = command.spawn().map_err(McpAdapterError::Start)?;
    let pid = child_pid_raw(&child).map_err(|error| McpAdapterError::Cleanup(error.to_string()))?;
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

    if let Err(source) = stdin.write_all(request).await {
        stop_and_capture(&mut child, pid, stdout, stderr).await?;
        return Err(McpAdapterError::Write(source));
    }
    if let Err(source) = stdin.shutdown().await {
        stop_and_capture(&mut child, pid, stdout, stderr).await?;
        return Err(McpAdapterError::Write(source));
    }
    drop(stdin);

    let signal = {
        let wait = child.wait();
        tokio::pin!(wait);
        let deadline = tokio::time::sleep(PROCESS_DEADLINE);
        tokio::pin!(deadline);
        tokio::select! {
            status = &mut wait => ProcessSignal::Exited(status),
            terminal = &mut terminal_receiver => if terminal.is_ok() {
                ProcessSignal::Terminal
            } else {
                tokio::select! {
                    status = &mut wait => ProcessSignal::Exited(status),
                    () = &mut deadline => ProcessSignal::Deadline,
                }
            },
            () = &mut deadline => ProcessSignal::Deadline,
        }
    };
    match signal {
        ProcessSignal::Exited(Err(source)) => {
            stop_and_capture(&mut child, pid, stdout, stderr).await?;
            Err(McpAdapterError::Wait(source))
        }
        ProcessSignal::Exited(Ok(status)) => {
            let (stdout, stderr) = stop_and_capture(&mut child, pid, stdout, stderr).await?;
            parse_captured(&stdout, &stderr, &format!("{status}"))
        }
        ProcessSignal::Terminal => {
            let (stdout, stderr) = stop_and_capture(&mut child, pid, stdout, stderr).await?;
            parse_captured(&stdout, &stderr, "stopped after terminal record")
        }
        ProcessSignal::Deadline => {
            let (stdout, stderr) = stop_and_capture(&mut child, pid, stdout, stderr).await?;
            if stdout.bytes.contains(&b'\n') {
                parse_captured(&stdout, &stderr, "stopped at Host deadline")
            } else {
                Err(McpAdapterError::Timeout)
            }
        }
    }
}

fn parse_captured(
    stdout: &CapturedHead,
    stderr: &CapturedHead,
    status: &str,
) -> Result<AdapterCatalog, McpAdapterError> {
    if stdout.truncated {
        return Err(McpAdapterError::OutputLimit);
    }
    let terminal = parse_discovery_record(&stdout.bytes);
    match terminal {
        Ok(record) => record,
        Err(protocol) => {
            let diagnostic = String::from_utf8_lossy(&stderr.bytes);
            let suffix = if diagnostic.trim().is_empty() {
                String::new()
            } else {
                format!("; stderr: {}", diagnostic.trim())
            };
            Err(McpAdapterError::Protocol(format!(
                "{protocol}{suffix}; process status {status}"
            )))
        }
    }
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

enum ProcessSignal {
    Exited(io::Result<std::process::ExitStatus>),
    Terminal,
    Deadline,
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
            .map(|record| Err(McpAdapterError::Remote(record.failure)))
            .map_err(|error| format!("decode failed record: {error}")),
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

#[cfg(test)]
mod tests {
    use std::{fs, time::Duration};

    use tempfile::tempdir;

    use super::{McpAdapterError, McpHostError, discover, parse_discovery_record};
    use crate::mcp::{McpFailureKind, McpOutcomeCertainty};

    #[test]
    fn typed_remote_failure_survives_the_process_boundary() {
        let parsed = parse_discovery_record(
            br#"{"wire_version":1,"event":"failed","failure":{"kind":"incompatible_protocol","certainty":"definite","message":"wrong revision","partial_changes_possible":false,"diagnostic":{"code":"protocol_version_mismatch","http_status":409,"detail":"server omitted the pinned revision"}}}
"#,
        )
        .expect("valid terminal record");
        let Err(McpAdapterError::Remote(failure)) = parsed else {
            panic!("expected typed remote failure");
        };

        assert_eq!(failure.kind(), McpFailureKind::IncompatibleProtocol);
        assert_eq!(failure.certainty(), McpOutcomeCertainty::Definite);
        assert!(!failure.partial_changes_possible());
        assert_eq!(failure.diagnostic_code(), Some("protocol_version_mismatch"));
        assert_eq!(failure.diagnostic_http_status(), Some(409));
    }

    #[test]
    fn unknown_failure_class_is_not_accepted_as_a_wire_v1_record() {
        let error = parse_discovery_record(
            br#"{"wire_version":1,"event":"failed","failure":{"kind":"maybe","certainty":"definite","message":"ambiguous","partial_changes_possible":false,"diagnostic":{"detail":"invalid class"}}}
"#,
        )
        .expect_err("wire v1 failure classes are closed");

        assert!(error.contains("decode failed record"));
    }

    #[test]
    fn discovery_accepts_exactly_one_terminal_record() {
        let record = br#"{"wire_version":1,"event":"failed","failure":{"kind":"protocol","certainty":"definite","message":"bad","partial_changes_possible":false,"diagnostic":{"detail":"bad"}}}
"#;
        let mut duplicated = record.to_vec();
        duplicated.extend_from_slice(record);

        assert_eq!(
            parse_discovery_record(&duplicated).expect_err("duplicate terminal record"),
            "adapter returned more than one discovery record"
        );
    }

    #[tokio::test]
    async fn a_valid_terminal_catalog_survives_hung_adapter_cleanup() {
        let directory = tempdir().expect("temporary adapter directory");
        let adapter = directory.path().join("adapter.mjs");
        fs::write(
            &adapter,
            r#"
const terminal = {
  wire_version: 1,
  event: "discovered",
  catalog: {
    endpoint: "https://example.com/mcp",
    protocol_version: "2026-07-28",
    adapter_revision: "mcp-client-node-v0.1.0",
    tools: [],
    rejected_tools: []
  }
};
process.stdout.write(`${JSON.stringify(terminal)}\n`);
await new Promise(() => {});
"#,
        )
        .expect("write hanging adapter");

        let snapshot = tokio::time::timeout(
            Duration::from_secs(3),
            discover(&adapter, "primary", "https://example.com/mcp"),
        )
        .await
        .expect("terminal should stop hung cleanup promptly")
        .expect("preserve valid terminal catalog");

        assert_eq!(snapshot.connection_id(), "primary");
        assert!(snapshot.tools().is_empty());
    }

    #[tokio::test]
    async fn records_after_a_terminal_are_rejected_at_the_real_process_boundary() {
        let directory = tempdir().expect("temporary adapter directory");
        let adapter = directory.path().join("adapter.mjs");
        fs::write(
            &adapter,
            r#"
const terminal = {
  wire_version: 1,
  event: "discovered",
  catalog: {
    endpoint: "https://example.com/mcp",
    protocol_version: "2026-07-28",
    adapter_revision: "mcp-client-node-v0.1.0",
    tools: [],
    rejected_tools: []
  }
};
const line = JSON.stringify(terminal);
process.stdout.write(`${line}\n${line}\n`);
await new Promise(() => {});
"#,
        )
        .expect("write duplicate-terminal adapter");

        let error = tokio::time::timeout(
            Duration::from_secs(3),
            discover(&adapter, "primary", "https://example.com/mcp"),
        )
        .await
        .expect("duplicate terminal should stop the process promptly")
        .expect_err("duplicate terminal must fail");
        let McpHostError::Adapter(McpAdapterError::Protocol(message)) = error else {
            panic!("expected process protocol failure");
        };

        assert!(message.contains("more than one discovery record"));
    }
}
