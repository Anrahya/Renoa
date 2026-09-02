use std::{io, path::PathBuf, time::Duration};

use tokio::{
    io::{AsyncRead, AsyncReadExt as _, AsyncWriteExt as _},
    process::Command,
    task::JoinHandle,
};
use tokio_util::sync::CancellationToken;

use crate::{
    mcp::McpCredentialError,
    process::{child_pid_raw, configure_process_group, stop_process_group_raw},
};

const SOURCE: &str = "Secret Service";
const DEADLINE: Duration = Duration::from_secs(15);
const MAX_OUTPUT_BYTES: usize = 768 * 1_024;
const MAX_STDERR_BYTES: usize = 4 * 1_024;

#[derive(Clone)]
pub(super) struct SecretService {
    executable: PathBuf,
}

impl SecretService {
    pub(super) const fn new(executable: PathBuf) -> Self {
        Self { executable }
    }

    pub(super) async fn lookup(
        &self,
        credential_id: &str,
        cancellation: CancellationToken,
    ) -> Result<Option<Vec<u8>>, McpCredentialError> {
        run(
            &self.executable,
            SecretCommand::Lookup { credential_id },
            cancellation,
        )
        .await
    }

    pub(super) async fn store_bytes(
        &self,
        credential_id: &str,
        bytes: &[u8],
        cancellation: CancellationToken,
    ) -> Result<(), McpCredentialError> {
        let result = run(
            &self.executable,
            SecretCommand::Store {
                credential_id,
                bytes,
            },
            cancellation,
        )
        .await;
        result?.ok_or(McpCredentialError::InvalidOutput(SOURCE))?;
        Ok(())
    }
}

#[derive(Clone, Copy)]
enum SecretCommand<'a> {
    Lookup {
        credential_id: &'a str,
    },
    Store {
        credential_id: &'a str,
        bytes: &'a [u8],
    },
}

struct PreparedSecretCommand<'a> {
    arguments: Vec<&'a str>,
    input: Option<&'a [u8]>,
    missing_is_none: bool,
}

fn prepare(request: SecretCommand<'_>) -> PreparedSecretCommand<'_> {
    match request {
        SecretCommand::Lookup { credential_id } => PreparedSecretCommand {
            arguments: vec![
                "lookup",
                "application",
                "renoa",
                "credential",
                credential_id,
            ],
            input: None,
            missing_is_none: true,
        },
        SecretCommand::Store {
            credential_id,
            bytes,
        } => PreparedSecretCommand {
            arguments: vec![
                "store",
                "--label=Renoa MCP OAuth",
                "application",
                "renoa",
                "credential",
                credential_id,
            ],
            input: Some(bytes),
            missing_is_none: false,
        },
    }
}

async fn run(
    executable: &std::path::Path,
    request: SecretCommand<'_>,
    cancellation: CancellationToken,
) -> Result<Option<Vec<u8>>, McpCredentialError> {
    if cancellation.is_cancelled() {
        return Err(McpCredentialError::Cancelled);
    }
    let PreparedSecretCommand {
        arguments,
        input,
        missing_is_none,
    } = prepare(request);
    let mut command = Command::new(executable);
    command
        .args(arguments)
        .stdin(if input.is_some() {
            std::process::Stdio::piped()
        } else {
            std::process::Stdio::null()
        })
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());
    configure_process_group(&mut command);
    let mut child = command
        .spawn()
        .map_err(|source| McpCredentialError::Start {
            source_name: SOURCE,
            source,
        })?;
    let pid = match child_pid_raw(&child) {
        Ok(pid) => pid,
        Err(error) => {
            child
                .kill()
                .await
                .map_err(|cleanup| McpCredentialError::Cleanup {
                    source_name: SOURCE,
                    detail: cleanup.to_string(),
                })?;
            return Err(McpCredentialError::Cleanup {
                source_name: SOURCE,
                detail: format!("credential process has no identity: {error}"),
            });
        }
    };
    let (Some(stdout), Some(stderr)) = (child.stdout.take(), child.stderr.take()) else {
        stop_process_group_raw(&mut child, pid)
            .await
            .map_err(cleanup_error)?;
        return Err(McpCredentialError::MissingPipe(SOURCE));
    };
    let stdout = drain(stdout, MAX_OUTPUT_BYTES.saturating_add(1));
    let stderr = drain(stderr, MAX_STDERR_BYTES);
    let deadline = tokio::time::Instant::now() + DEADLINE;
    if let Some(bytes) = input {
        let Some(mut stdin) = child.stdin.take() else {
            stop_and_join(&mut child, pid, stdout, stderr).await?;
            return Err(McpCredentialError::MissingPipe(SOURCE));
        };
        let write = async {
            stdin.write_all(bytes).await?;
            stdin.shutdown().await
        };
        tokio::pin!(write);
        let write_result = tokio::select! {
            biased;
            result = &mut write => Some(result),
            () = cancellation.cancelled() => None,
            () = tokio::time::sleep_until(deadline) => None,
        };
        drop(stdin);
        match write_result {
            Some(Ok(())) => {}
            Some(Err(source)) => {
                stop_and_join(&mut child, pid, stdout, stderr).await?;
                return Err(McpCredentialError::Write {
                    source_name: SOURCE,
                    source,
                });
            }
            None => {
                stop_and_join(&mut child, pid, stdout, stderr).await?;
                return if cancellation.is_cancelled() {
                    Err(McpCredentialError::Cancelled)
                } else {
                    Err(McpCredentialError::Timeout(SOURCE))
                };
            }
        }
    }
    let signal = tokio::select! {
        biased;
        () = cancellation.cancelled() => Signal::Cancelled,
        () = tokio::time::sleep_until(deadline) => Signal::Deadline,
        status = child.wait() => Signal::Exited(status),
    };
    let (stdout, stderr) = stop_and_join(&mut child, pid, stdout, stderr).await?;
    classify(signal, stdout, stderr, missing_is_none)
}

fn classify(
    signal: Signal,
    mut stdout: BoundedOutput,
    mut stderr: BoundedOutput,
    missing_is_none: bool,
) -> Result<Option<Vec<u8>>, McpCredentialError> {
    let stderr_empty = stderr.bytes.iter().all(u8::is_ascii_whitespace);
    stderr.bytes.fill(0);
    if stdout.truncated {
        stdout.bytes.fill(0);
        return Err(McpCredentialError::OutputLimit(SOURCE));
    }
    match signal {
        Signal::Cancelled => {
            stdout.bytes.fill(0);
            Err(McpCredentialError::Cancelled)
        }
        Signal::Deadline => {
            stdout.bytes.fill(0);
            Err(McpCredentialError::Timeout(SOURCE))
        }
        Signal::Exited(Err(source)) => {
            stdout.bytes.fill(0);
            Err(McpCredentialError::Wait {
                source_name: SOURCE,
                source,
            })
        }
        Signal::Exited(Ok(status)) if status.success() => {
            Ok(Some(std::mem::take(&mut stdout.bytes)))
        }
        Signal::Exited(Ok(_)) if missing_is_none && stderr_empty => {
            stdout.bytes.fill(0);
            Ok(None)
        }
        Signal::Exited(Ok(status)) => {
            stdout.bytes.fill(0);
            Err(McpCredentialError::Unavailable {
                source_name: SOURCE,
                reference: "the MCP OAuth credential bundle".to_owned(),
                status: status.to_string(),
                guidance: "unlock the desktop keyring and try again".to_owned(),
            })
        }
    }
}

async fn stop_and_join(
    child: &mut tokio::process::Child,
    pid: u32,
    stdout: JoinHandle<io::Result<BoundedOutput>>,
    stderr: JoinHandle<io::Result<BoundedOutput>>,
) -> Result<(BoundedOutput, BoundedOutput), McpCredentialError> {
    let cleanup = stop_process_group_raw(child, pid)
        .await
        .map_err(cleanup_error);
    let (stdout, stderr) = tokio::join!(stdout, stderr);
    cleanup?;
    Ok((joined(stdout, "stdout")?, joined(stderr, "stderr")?))
}

fn cleanup_error(error: impl std::fmt::Display) -> McpCredentialError {
    McpCredentialError::Cleanup {
        source_name: SOURCE,
        detail: error.to_string(),
    }
}

enum Signal {
    Exited(io::Result<std::process::ExitStatus>),
    Cancelled,
    Deadline,
}

struct BoundedOutput {
    bytes: Vec<u8>,
    truncated: bool,
}

impl Drop for BoundedOutput {
    fn drop(&mut self) {
        self.bytes.fill(0);
    }
}

fn drain(
    mut reader: impl AsyncRead + Unpin + Send + 'static,
    limit: usize,
) -> JoinHandle<io::Result<BoundedOutput>> {
    tokio::spawn(async move {
        let mut bytes = Vec::new();
        let mut buffer = [0_u8; 4_096];
        let mut truncated = false;
        loop {
            let read = reader.read(&mut buffer).await?;
            if read == 0 {
                return Ok(BoundedOutput { bytes, truncated });
            }
            let retained = read.min(limit.saturating_sub(bytes.len()));
            bytes.extend_from_slice(&buffer[..retained]);
            truncated |= retained < read;
        }
    })
}

fn joined(
    result: Result<io::Result<BoundedOutput>, tokio::task::JoinError>,
    stream: &'static str,
) -> Result<BoundedOutput, McpCredentialError> {
    result
        .map_err(|source| McpCredentialError::ReaderTask {
            source_name: SOURCE,
            stream,
            source,
        })?
        .map_err(|source| McpCredentialError::Read {
            source_name: SOURCE,
            stream,
            source,
        })
}
