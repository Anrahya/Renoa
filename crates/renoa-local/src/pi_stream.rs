use std::{
    collections::BTreeMap,
    future::Future as _,
    num::NonZeroU32,
    pin::Pin,
    task::{Context, Poll},
    time::Duration,
};

use futures_util::{Stream, StreamExt as _, stream};
use renoa_agent::{
    AssistantDelta, ModelError, ModelEvent, ModelEventStream, ModelRequest, ModelResponse,
};
use serde::Deserialize;
use tokio::{
    io::{AsyncBufReadExt as _, AsyncReadExt as _, AsyncWriteExt as _, BufReader},
    sync::mpsc,
};
use tokio_util::sync::CancellationToken;

use crate::pi_model::{
    OUTPUT_LIMIT, PiBridgeConfig, decode_response, drain, join_output, model_error,
    stop_bridge_child,
};
use crate::process::child_pid_raw;

const FIRST_OUTPUT_DEADLINE: Duration = Duration::from_mins(5);
const STREAM_IDLE_DEADLINE: Duration = Duration::from_mins(5);
const MODEL_TOTAL_DEADLINE: Duration = Duration::from_mins(30);

pub(crate) fn stream_model(
    config: PiBridgeConfig,
    max_output_tokens: NonZeroU32,
    request: &ModelRequest,
    cancellation: CancellationToken,
) -> ModelEventStream<'static> {
    let input = match serde_json::to_vec(&request) {
        Ok(input) => input,
        Err(error) => {
            return stream::once(async move { Err(model_error("encode Pi model request", error)) })
                .boxed();
        }
    };
    let (sender, receiver) = mpsc::channel(1);
    let stream_cancellation = cancellation.clone();
    let task = tokio::spawn({
        let errors = sender.clone();
        async move {
            if let Err(error) =
                run_stream(config, max_output_tokens, input, cancellation, sender).await
            {
                let _ = errors.send(Err(error)).await;
            }
        }
    });
    Box::pin(PiEventStream {
        receiver,
        cancellation: stream_cancellation,
        task: Some(task),
    })
}

struct PiEventStream {
    receiver: mpsc::Receiver<Result<ModelEvent, ModelError>>,
    cancellation: CancellationToken,
    task: Option<tokio::task::JoinHandle<()>>,
}

impl Stream for PiEventStream {
    type Item = Result<ModelEvent, ModelError>;

    fn poll_next(mut self: Pin<&mut Self>, context: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        match self.receiver.poll_recv(context) {
            Poll::Ready(Some(event)) => Poll::Ready(Some(event)),
            Poll::Pending => Poll::Pending,
            Poll::Ready(None) => {
                let Some(task) = self.task.as_mut() else {
                    return Poll::Ready(None);
                };
                match Pin::new(task).poll(context) {
                    Poll::Pending => Poll::Pending,
                    Poll::Ready(Ok(())) => {
                        self.task = None;
                        Poll::Ready(None)
                    }
                    Poll::Ready(Err(error)) => {
                        self.task = None;
                        Poll::Ready(Some(Err(ModelError::new(format!(
                            "Pi model stream task failed: {error}"
                        )))))
                    }
                }
            }
        }
    }
}

impl Drop for PiEventStream {
    fn drop(&mut self) {
        self.cancellation.cancel();
    }
}

async fn run_stream(
    config: PiBridgeConfig,
    max_output_tokens: NonZeroU32,
    input: Vec<u8>,
    cancellation: CancellationToken,
    sender: mpsc::Sender<Result<ModelEvent, ModelError>>,
) -> Result<(), ModelError> {
    let mut child = config
        .command("stream", Some(max_output_tokens))
        .spawn()
        .map_err(|error| model_error("start Pi model bridge", error))?;
    let pid = child_pid_raw(&child)
        .map_err(|error| ModelError::new(format!("Pi model bridge ownership failed: {error}")))?;
    let mut stdin = child
        .stdin
        .take()
        .ok_or_else(|| ModelError::new("Pi model bridge stdin is unavailable"))?;
    let writer = tokio::spawn(async move { stdin.write_all(&input).await });
    let stdout = child.stdout.take().expect("piped stdout");
    let stderr = drain(child.stderr.take().expect("piped stderr"));
    let total_deadline = tokio::time::Instant::now() + MODEL_TOTAL_DEADLINE;
    let read = read_records(
        stdout,
        &cancellation,
        &sender,
        StreamDeadlines::production(total_deadline),
    )
    .await;
    let terminal = match read {
        Ok(ReadExit::Finished(terminal)) => terminal,
        Ok(ReadExit::Cancelled) => {
            stop_bridge_child(&mut child, pid).await?;
            drain_after_stop(writer, stderr).await;
            return Err(ModelError::new("Pi model request was cancelled"));
        }
        Ok(ReadExit::ReceiverClosed) => {
            stop_bridge_child(&mut child, pid).await?;
            drain_after_stop(writer, stderr).await;
            return Ok(());
        }
        Err(error) => {
            stop_bridge_child(&mut child, pid).await?;
            drain_after_stop(writer, stderr).await;
            return Err(error);
        }
    };
    let status = tokio::select! {
        biased;
        () = cancellation.cancelled() => {
            stop_bridge_child(&mut child, pid).await?;
            drain_after_stop(writer, stderr).await;
            return Err(ModelError::new("Pi model request was cancelled"));
        }
        () = sender.closed() => {
            stop_bridge_child(&mut child, pid).await?;
            drain_after_stop(writer, stderr).await;
            return Ok(());
        }
        status = child.wait() => {
            status.map_err(|error| model_error("wait for Pi model bridge", error))?
        }
        () = tokio::time::sleep_until(total_deadline) => {
            stop_bridge_child(&mut child, pid).await?;
            drain_after_stop(writer, stderr).await;
            return Err(ModelError::timeout(
                "model invocation exceeded its 30-minute total deadline",
            ));
        }
    };
    stop_bridge_child(&mut child, pid).await?;
    writer
        .await
        .map_err(|error| ModelError::new(format!("Pi request writer failed: {error}")))?
        .map_err(|error| model_error("write Pi model request", error))?;
    let stderr = join_output(stderr, "stderr").await?;
    if !status.success() {
        return Err(ModelError::new(format!(
            "Pi model bridge exited with {status}: {}",
            String::from_utf8_lossy(&stderr.bytes)
        )));
    }
    if stderr.truncated {
        return Err(ModelError::new("Pi model bridge stderr exceeded 16 MiB"));
    }
    let response = (*terminal)?;
    let _ = sender.send(Ok(ModelEvent::Completed { response })).await;
    Ok(())
}

async fn read_records(
    stdout: impl tokio::io::AsyncRead + Unpin,
    cancellation: &CancellationToken,
    sender: &mpsc::Sender<Result<ModelEvent, ModelError>>,
    deadlines: StreamDeadlines,
) -> Result<ReadExit, ModelError> {
    let output_limit = u64::try_from(OUTPUT_LIMIT + 1).expect("16 MiB fits in u64");
    let mut lines = BufReader::new(stdout.take(output_limit)).lines();
    let mut state = StreamReadState::new(deadlines);
    loop {
        let line = tokio::select! {
            biased;
            () = cancellation.cancelled() => {
                return Ok(ReadExit::Cancelled);
            }
            () = sender.closed() => {
                return Ok(ReadExit::ReceiverClosed);
            }
            () = tokio::time::sleep_until(state.deadlines.total) => {
                return Err(ModelError::timeout(
                    "model invocation exceeded its 30-minute total deadline",
                ));
            }
            () = tokio::time::sleep_until(state.deadlines.first_output), if !state.first_output_seen => {
                return Err(ModelError::timeout(
                    "model produced no output within its 5-minute first-output deadline",
                ));
            }
            () = tokio::time::sleep_until(state.idle_deadline) => {
                return Err(ModelError::timeout(
                    "model stream was idle for 5 minutes",
                ));
            }
            line = lines.next_line() => line.map_err(|error| model_error("read Pi model stream", error))?,
        };
        let Some(line) = line else {
            break;
        };
        if let Some(event) = state.accept_line(line.as_bytes())?
            && let Some(exit) = forward_delta(sender, cancellation, event).await
        {
            return Ok(exit);
        }
    }
    state.finish()
}

#[derive(Clone, Copy)]
struct StreamDeadlines {
    first_output: tokio::time::Instant,
    idle: Duration,
    total: tokio::time::Instant,
}

impl StreamDeadlines {
    fn production(total: tokio::time::Instant) -> Self {
        Self {
            first_output: tokio::time::Instant::now() + FIRST_OUTPUT_DEADLINE,
            idle: STREAM_IDLE_DEADLINE,
            total,
        }
    }
}

struct StreamReadState {
    deadlines: StreamDeadlines,
    idle_deadline: tokio::time::Instant,
    received_bytes: usize,
    first_output_seen: bool,
    terminal: Option<Box<Result<ModelResponse, ModelError>>>,
}

impl StreamReadState {
    fn new(deadlines: StreamDeadlines) -> Self {
        Self {
            deadlines,
            idle_deadline: tokio::time::Instant::now() + deadlines.idle,
            received_bytes: 0,
            first_output_seen: false,
            terminal: None,
        }
    }

    fn accept_line(&mut self, encoded: &[u8]) -> Result<Option<ModelEvent>, ModelError> {
        self.received_bytes = self
            .received_bytes
            .checked_add(encoded.len())
            .and_then(|bytes| bytes.checked_add(1))
            .ok_or_else(|| ModelError::new("Pi model response size overflowed usize"))?;
        if self.received_bytes > OUTPUT_LIMIT {
            return Err(ModelError::new("Pi model response exceeded 16 MiB"));
        }
        if self.terminal.is_some() {
            return Err(ModelError::new(
                "Pi model bridge emitted data after its terminal record",
            ));
        }
        self.idle_deadline = tokio::time::Instant::now() + self.deadlines.idle;
        Ok(match decode_record(encoded)? {
            StreamRecord::ProviderRequest { payload } => {
                Some(ModelEvent::ProviderRequest { payload })
            }
            StreamRecord::ProviderResponse { status, headers } => {
                Some(ModelEvent::ProviderResponse { status, headers })
            }
            StreamRecord::ContentDelta {
                content_index,
                delta,
            } => {
                self.first_output_seen = true;
                Some(ModelEvent::ContentDelta {
                    content_index,
                    delta: delta.into(),
                })
            }
            StreamRecord::Completed { response } => {
                self.first_output_seen = true;
                self.terminal = Some(Box::new(Ok(response)));
                None
            }
            StreamRecord::Error { error, error_kind } => {
                self.first_output_seen = true;
                self.terminal = Some(Box::new(Err(match error_kind {
                    Some(BridgeErrorKind::ContextWindowExceeded) => {
                        ModelError::context_window_exceeded(error)
                    }
                    Some(BridgeErrorKind::AuthenticationFailed) => {
                        ModelError::authentication_failed(error)
                    }
                    None => ModelError::new(error),
                })));
                None
            }
        })
    }

    fn finish(self) -> Result<ReadExit, ModelError> {
        let terminal = self.terminal.ok_or_else(|| {
            ModelError::new("Pi model bridge closed without a terminal stream record")
        })?;
        Ok(ReadExit::Finished(terminal))
    }
}

async fn forward_delta(
    sender: &mpsc::Sender<Result<ModelEvent, ModelError>>,
    cancellation: &CancellationToken,
    event: ModelEvent,
) -> Option<ReadExit> {
    tokio::select! {
        biased;
        () = cancellation.cancelled() => Some(ReadExit::Cancelled),
        () = sender.closed() => Some(ReadExit::ReceiverClosed),
        result = sender.send(Ok(event)) => result.err().map(|_| ReadExit::ReceiverClosed),
    }
}

async fn drain_after_stop(
    writer: tokio::task::JoinHandle<std::io::Result<()>>,
    stderr: tokio::task::JoinHandle<std::io::Result<crate::pi_model::CapturedOutput>>,
) {
    let _ = writer.await;
    let _ = join_output(stderr, "stderr").await;
}

#[derive(Debug)]
enum ReadExit {
    Finished(Box<Result<ModelResponse, ModelError>>),
    Cancelled,
    ReceiverClosed,
}

fn decode_record(encoded: &[u8]) -> Result<StreamRecord, ModelError> {
    let value: serde_json::Value = serde_json::from_slice(encoded)
        .map_err(|error| model_error("decode Pi model stream record", error))?;
    if value.get("event").is_some() {
        serde_json::from_value(value)
            .map_err(|error| model_error("decode Pi model stream record", error))
    } else {
        decode_response(encoded).map(|response| StreamRecord::Completed { response })
    }
}

#[derive(Deserialize)]
#[serde(tag = "event", rename_all = "snake_case")]
enum StreamRecord {
    ProviderRequest {
        payload: serde_json::Value,
    },
    ProviderResponse {
        status: u16,
        headers: BTreeMap<String, String>,
    },
    ContentDelta {
        content_index: usize,
        delta: BridgeDelta,
    },
    Completed {
        response: ModelResponse,
    },
    Error {
        error: String,
        error_kind: Option<BridgeErrorKind>,
    },
}

#[derive(Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum BridgeDelta {
    Text { text: String },
    Reasoning { text: String },
    ToolCallStart { id: String, name: String },
    ToolCallArguments { json_delta: String },
}

impl From<BridgeDelta> for AssistantDelta {
    fn from(delta: BridgeDelta) -> Self {
        match delta {
            BridgeDelta::Text { text } => Self::Text { text },
            BridgeDelta::Reasoning { text } => Self::Reasoning { text },
            BridgeDelta::ToolCallStart { id, name } => Self::ToolCallStart { id, name },
            BridgeDelta::ToolCallArguments { json_delta } => Self::ToolCallArguments { json_delta },
        }
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "snake_case")]
enum BridgeErrorKind {
    ContextWindowExceeded,
    AuthenticationFailed,
}

#[cfg(test)]
#[path = "pi_stream_tests.rs"]
mod tests;
