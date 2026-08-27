use std::{
    io,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};

use tokio::{
    io::{AsyncRead, AsyncReadExt as _},
    sync::oneshot,
    task::JoinHandle,
};

use super::{MAX_STDOUT_BYTES, WIRE_VERSION, wire::RecordHeader};
use crate::process::stop_process_group_raw;

pub(super) struct ProcessCapture {
    pub(super) stdout: CapturedOutput,
    pub(super) stderr: CapturedOutput,
    pub(super) cleanup_error: Option<String>,
}

pub(super) async fn stop_and_capture(
    child: &mut tokio::process::Child,
    pid: u32,
    stdout: JoinHandle<CapturedOutput>,
    stderr: JoinHandle<CapturedOutput>,
) -> ProcessCapture {
    let cleanup = stop_process_group_raw(child, pid);
    let (cleanup, stdout, stderr) = tokio::join!(cleanup, stdout, stderr);
    ProcessCapture {
        stdout: joined_output(stdout, "stdout"),
        stderr: joined_output(stderr, "stderr"),
        cleanup_error: cleanup.err().map(|error| error.to_string()),
    }
}

pub(super) struct CapturedOutput {
    pub(super) bytes: Vec<u8>,
    pub(super) truncated: bool,
    pub(super) read_error: Option<io::Error>,
}

impl Drop for CapturedOutput {
    fn drop(&mut self) {
        self.bytes.fill(0);
    }
}

pub(super) fn drain_stdout(
    reader: impl AsyncRead + Unpin + Send + 'static,
    dispatch_started: Arc<AtomicBool>,
    terminal_sender: oneshot::Sender<()>,
) -> JoinHandle<CapturedOutput> {
    drain_with_observer(
        reader,
        MAX_STDOUT_BYTES,
        Some((dispatch_started, terminal_sender)),
    )
}

pub(super) fn drain_output(
    reader: impl AsyncRead + Unpin + Send + 'static,
    limit: usize,
) -> JoinHandle<CapturedOutput> {
    drain_with_observer(reader, limit, None)
}

fn drain_with_observer(
    mut reader: impl AsyncRead + Unpin + Send + 'static,
    limit: usize,
    observer: Option<(Arc<AtomicBool>, oneshot::Sender<()>)>,
) -> JoinHandle<CapturedOutput> {
    tokio::spawn(async move {
        let mut bytes = Vec::new();
        let mut buffer = [0_u8; 8_192];
        let mut truncated = false;
        let mut inspected = 0;
        let mut observer = observer.map(|(dispatch, terminal)| (dispatch, Some(terminal)));
        loop {
            let read = match reader.read(&mut buffer).await {
                Ok(read) => read,
                Err(error) => {
                    return CapturedOutput {
                        bytes,
                        truncated,
                        read_error: Some(error),
                    };
                }
            };
            if read == 0 {
                break;
            }
            let retained = read.min(limit.saturating_sub(bytes.len()));
            bytes.extend_from_slice(&buffer[..retained]);
            truncated |= retained < read;
            while let Some(newline) = bytes[inspected..].iter().position(|byte| *byte == b'\n') {
                let end = inspected + newline;
                if let Some((dispatch, terminal)) = observer.as_mut()
                    && let Ok(header) =
                        serde_json::from_slice::<RecordHeader>(&bytes[inspected..end])
                {
                    match header.event.as_str() {
                        "dispatch_started" if header.wire_version == WIRE_VERSION => {
                            dispatch.store(true, Ordering::Release);
                        }
                        "completed" | "failed" if header.wire_version == WIRE_VERSION => {
                            if let Some(sender) = terminal.take() {
                                let _receiver_closed = sender.send(()).is_err();
                            }
                        }
                        _ => {}
                    }
                }
                inspected = end + 1;
            }
        }
        CapturedOutput {
            bytes,
            truncated,
            read_error: None,
        }
    })
}

fn joined_output(
    result: Result<CapturedOutput, tokio::task::JoinError>,
    stream: &'static str,
) -> CapturedOutput {
    result.unwrap_or_else(|error| CapturedOutput {
        bytes: Vec::new(),
        truncated: false,
        read_error: Some(io::Error::other(format!(
            "MCP adapter {stream} reader task failed: {error}"
        ))),
    })
}
