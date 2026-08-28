use std::{
    io,
    path::Path,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};

mod capture;
mod wire;

#[cfg(test)]
mod tests;

use serde::Serialize;
use serde_json::Value;
use tokio::{io::AsyncWriteExt as _, process::Command, sync::oneshot};
use tokio_util::sync::CancellationToken;

use self::{
    capture::{CapturedOutput, drain_output, drain_stdout, stop_and_capture},
    wire::{CallTerminal, McpCallResult, ParseFailure, parse_call_records},
};
use super::{
    AlphaMcpTool, McpAdapterError, McpAuthorization, McpOutcomeCertainty, McpRemoteFailure,
};
use crate::process::{child_pid_raw, configure_process_group, stop_process_group_raw};

const WIRE_VERSION: u32 = 5;
const PROCESS_DEADLINE: Duration = Duration::from_secs(125);
const MAX_REQUEST_BYTES: usize = 1_024 * 1_024;
const MAX_STDOUT_BYTES: usize = 20 * 1_024 * 1_024;
const MAX_STDERR_BYTES: usize = 64 * 1_024;

pub(super) const CALL_BOUNDARY_REVISION: &str =
    "rust-call-v1/wire-5/deadline-125s/request-1m/stdout-20m/stderr-64k/content-256";

#[derive(Debug)]
pub(super) struct McpCallFailure {
    source: McpAdapterError,
    certainty: McpOutcomeCertainty,
    partial_changes_possible: bool,
}

impl McpCallFailure {
    pub(super) fn into_parts(self) -> (McpAdapterError, McpOutcomeCertainty, bool) {
        (self.source, self.certainty, self.partial_changes_possible)
    }

    fn definite(source: McpAdapterError, partial_changes_possible: bool) -> Self {
        Self {
            source,
            certainty: McpOutcomeCertainty::Definite,
            partial_changes_possible,
        }
    }

    fn unknown(source: McpAdapterError) -> Self {
        Self {
            source,
            certainty: McpOutcomeCertainty::Unknown,
            partial_changes_possible: true,
        }
    }

    fn remote(source: McpRemoteFailure) -> Self {
        Self {
            certainty: source.certainty(),
            partial_changes_possible: source.partial_changes_possible(),
            source: McpAdapterError::Remote(source),
        }
    }
}

pub(super) async fn call_tool(
    adapter: &Path,
    selected: &AlphaMcpTool,
    authorization: Option<&McpAuthorization>,
    arguments: &Value,
    cancellation: CancellationToken,
) -> Result<McpCallResult, McpCallFailure> {
    if cancellation.is_cancelled() {
        return Err(McpCallFailure::definite(McpAdapterError::Cancelled, false));
    }
    let mut request = serde_json::to_vec(&CallRequest {
        wire_version: WIRE_VERSION,
        action: "call",
        endpoint: selected.endpoint(),
        protocol_version: selected.protocol_version(),
        headers: selected.request_headers(),
        authorization: authorization.map(WireAuthorization::from),
        tool: FrozenTool {
            name: selected.tool().name(),
            input_schema: selected.tool().input_schema(),
            output_schema: selected.tool().output_schema(),
        },
        arguments,
    })
    .map_err(|error| McpCallFailure::definite(McpAdapterError::Encode(error), false))?;
    if request.len() > MAX_REQUEST_BYTES {
        return Err(McpCallFailure::definite(McpAdapterError::InputLimit, false));
    }

    let result = run_adapter(adapter, &request, authorization, cancellation).await;
    request.fill(0);
    result
}

#[derive(Serialize)]
struct CallRequest<'a> {
    wire_version: u32,
    action: &'static str,
    endpoint: &'a str,
    protocol_version: &'a str,
    #[serde(skip_serializing_if = "super::McpRequestHeaders::is_empty")]
    headers: &'a super::McpRequestHeaders,
    #[serde(skip_serializing_if = "Option::is_none")]
    authorization: Option<WireAuthorization<'a>>,
    tool: FrozenTool<'a>,
    arguments: &'a Value,
}

#[derive(Serialize)]
struct WireAuthorization<'a> {
    scheme: &'static str,
    token: &'a str,
}

impl<'a> From<&'a McpAuthorization> for WireAuthorization<'a> {
    fn from(authorization: &'a McpAuthorization) -> Self {
        Self {
            scheme: "bearer",
            token: authorization.bearer(),
        }
    }
}

#[derive(Serialize)]
struct FrozenTool<'a> {
    name: &'a str,
    input_schema: &'a Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    output_schema: Option<&'a Value>,
}

async fn run_adapter(
    adapter: &Path,
    request: &[u8],
    authorization: Option<&McpAuthorization>,
    cancellation: CancellationToken,
) -> Result<McpCallResult, McpCallFailure> {
    let deadline = tokio::time::Instant::now() + PROCESS_DEADLINE;
    let (mut child, pid) = spawn_adapter(adapter).await?;
    let pipes = (child.stdin.take(), child.stdout.take(), child.stderr.take());
    let (Some(mut stdin), Some(stdout), Some(stderr)) = pipes else {
        let failure = match stop_process_group_raw(&mut child, pid).await {
            Ok(()) => McpAdapterError::MissingPipe("configured standard-I/O pipe"),
            Err(error) => McpAdapterError::Cleanup(error.to_string()),
        };
        return Err(McpCallFailure::definite(failure, false));
    };

    let dispatch_started = Arc::new(AtomicBool::new(false));
    let (terminal_sender, mut terminal_receiver) = oneshot::channel();
    let stdout = drain_stdout(stdout, Arc::clone(&dispatch_started), terminal_sender);
    let stderr = drain_output(stderr, MAX_STDERR_BYTES);
    let write_signal = match write_request(&mut stdin, request, &cancellation, deadline).await {
        WriteSignal::Finished(Ok(())) => None,
        WriteSignal::Finished(Err(source)) => Some(ProcessSignal::Write(source)),
        WriteSignal::Cancelled => Some(ProcessSignal::Cancelled),
        WriteSignal::Deadline => Some(ProcessSignal::Deadline),
    };
    drop(stdin);
    if let Some(signal) = write_signal {
        let capture = stop_and_capture(&mut child, pid, stdout, stderr).await;
        return settle_capture(
            capture,
            signal,
            dispatch_started.load(Ordering::Acquire),
            authorization,
        );
    }

    let signal =
        wait_for_terminal(&mut child, &mut terminal_receiver, &cancellation, deadline).await;
    let capture = stop_and_capture(&mut child, pid, stdout, stderr).await;
    settle_capture(
        capture,
        signal,
        dispatch_started.load(Ordering::Acquire),
        authorization,
    )
}

async fn spawn_adapter(adapter: &Path) -> Result<(tokio::process::Child, u32), McpCallFailure> {
    let mut command = Command::new("node");
    command
        .arg("--dns-result-order=ipv4first")
        .arg(adapter)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());
    configure_process_group(&mut command);
    let mut child = command
        .spawn()
        .map_err(|error| McpCallFailure::definite(McpAdapterError::Start(error), false))?;
    let pid = match child_pid_raw(&child) {
        Ok(pid) => pid,
        Err(error) => {
            child.kill().await.map_err(|cleanup| {
                McpCallFailure::definite(McpAdapterError::Cleanup(cleanup.to_string()), false)
            })?;
            return Err(McpCallFailure::definite(
                McpAdapterError::Cleanup(error.to_string()),
                false,
            ));
        }
    };
    Ok((child, pid))
}

async fn write_request(
    stdin: &mut tokio::process::ChildStdin,
    request: &[u8],
    cancellation: &CancellationToken,
    deadline: tokio::time::Instant,
) -> WriteSignal {
    let write = async {
        stdin.write_all(request).await?;
        stdin.shutdown().await
    };
    tokio::pin!(write);
    tokio::select! {
        biased;
        result = &mut write => WriteSignal::Finished(result),
        () = cancellation.cancelled() => WriteSignal::Cancelled,
        () = tokio::time::sleep_until(deadline) => WriteSignal::Deadline,
    }
}

async fn wait_for_terminal(
    child: &mut tokio::process::Child,
    terminal_receiver: &mut oneshot::Receiver<()>,
    cancellation: &CancellationToken,
    deadline: tokio::time::Instant,
) -> ProcessSignal {
    let wait = child.wait();
    tokio::pin!(wait);
    tokio::select! {
        biased;
        terminal = terminal_receiver => if terminal.is_ok() {
            ProcessSignal::Terminal
        } else {
            tokio::select! {
                biased;
                () = cancellation.cancelled() => ProcessSignal::Cancelled,
                () = tokio::time::sleep_until(deadline) => ProcessSignal::Deadline,
                status = &mut wait => ProcessSignal::Exited(status),
            }
        },
        () = cancellation.cancelled() => ProcessSignal::Cancelled,
        () = tokio::time::sleep_until(deadline) => ProcessSignal::Deadline,
        status = &mut wait => ProcessSignal::Exited(status),
    }
}

enum WriteSignal {
    Finished(io::Result<()>),
    Cancelled,
    Deadline,
}

enum ProcessSignal {
    Write(io::Error),
    Exited(io::Result<std::process::ExitStatus>),
    Terminal,
    Cancelled,
    Deadline,
}

fn settle_capture(
    mut capture: capture::ProcessCapture,
    signal: ProcessSignal,
    observed_dispatch: bool,
    authorization: Option<&McpAuthorization>,
) -> Result<McpCallResult, McpCallFailure> {
    let mut parsed = parse_call_records(&capture.stdout.bytes);
    if let Ok(parsed) = &mut parsed
        && let Some(terminal) = &mut parsed.terminal
    {
        let settled = match terminal {
            CallTerminal::Completed(result) => {
                result.redact_authorization(authorization);
                Ok(result.clone())
            }
            CallTerminal::Failed(failure) => {
                failure.redact_authorization(authorization);
                Err(McpCallFailure::remote(failure.clone()))
            }
        };
        capture.stdout.bytes.fill(0);
        capture.stderr.bytes.fill(0);
        return settled;
    }

    let parsed_dispatch = match &parsed {
        Ok(parsed) => parsed.dispatch_started,
        Err(error) => error.dispatch_started,
    };
    let dispatch_started = observed_dispatch || parsed_dispatch;
    let terminal_attempted = matches!(
        &parsed,
        Err(ParseFailure {
            definite_terminal_evidence: true,
            ..
        })
    );
    let partial_changes_possible = dispatch_started || terminal_attempted;

    let source = if let Some(cleanup) = capture.cleanup_error {
        McpAdapterError::Cleanup(cleanup)
    } else if let Some(error) = capture.stdout.read_error.take() {
        McpAdapterError::Read {
            stream: "stdout",
            source: error,
        }
    } else if capture.stdout.truncated {
        McpAdapterError::OutputLimit
    } else if let Err(error) = parsed {
        McpAdapterError::Protocol(with_stderr(
            &error.message,
            &capture.stderr,
            "invalid terminal stream",
            authorization,
        ))
    } else {
        match signal {
            ProcessSignal::Write(source) => McpAdapterError::Write(source),
            ProcessSignal::Exited(Err(source)) => McpAdapterError::Wait(source),
            ProcessSignal::Exited(Ok(status)) => McpAdapterError::Protocol(with_stderr(
                "adapter returned no terminal record",
                &capture.stderr,
                &status.to_string(),
                authorization,
            )),
            ProcessSignal::Terminal => McpAdapterError::Protocol(with_stderr(
                "adapter announced an invalid terminal record",
                &capture.stderr,
                "stopped after terminal announcement",
                authorization,
            )),
            ProcessSignal::Cancelled => McpAdapterError::Cancelled,
            ProcessSignal::Deadline => McpAdapterError::Timeout,
        }
    };

    capture.stdout.bytes.fill(0);
    capture.stderr.bytes.fill(0);
    if dispatch_started && !terminal_attempted {
        Err(McpCallFailure::unknown(source))
    } else {
        Err(McpCallFailure::definite(source, partial_changes_possible))
    }
}

fn with_stderr(
    message: &str,
    stderr: &CapturedOutput,
    status: &str,
    authorization: Option<&McpAuthorization>,
) -> String {
    let mut diagnostic = String::from_utf8_lossy(&stderr.bytes).into_owned();
    if let Some(authorization) = authorization {
        authorization.redact_text(&mut diagnostic);
    }
    let suffix = if diagnostic.trim().is_empty() {
        String::new()
    } else {
        format!("; stderr: {}", diagnostic.trim())
    };
    let truncation = if stderr.truncated {
        "; stderr was truncated"
    } else {
        ""
    };
    format!("{message}{suffix}{truncation}; process status {status}")
}
