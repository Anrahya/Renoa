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
    AssistantDelta, InferenceOutcome, ModelError, ModelErrorKind, ModelEvent, ModelEventStream,
    ModelFailureDiagnostic, ModelRequest, ModelResponse,
};
use serde::Deserialize;
use tokio::{
    io::{AsyncBufReadExt as _, AsyncReadExt as _, AsyncWriteExt as _, BufReader},
    sync::mpsc,
};
use tokio_util::sync::CancellationToken;

use crate::model_bridge::{
    ModelBridgeConfig, OUTPUT_LIMIT, classified_error, decode_response, drain, join_output,
    model_error, stop_bridge_child,
};
use crate::process::child_pid_raw;

const FIRST_OUTPUT_DEADLINE: Duration = Duration::from_mins(5);
const STREAM_IDLE_DEADLINE: Duration = Duration::from_mins(5);
const MODEL_TOTAL_DEADLINE: Duration = Duration::from_mins(30);

pub(crate) fn stream_model(
    config: ModelBridgeConfig,
    max_output_tokens: NonZeroU32,
    request: &ModelRequest,
    cancellation: CancellationToken,
) -> ModelEventStream<'static> {
    let input = match serde_json::to_vec(&request) {
        Ok(input) => input,
        Err(error) => {
            return stream::once(async move { Err(model_error("encode model request", error)) })
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
    Box::pin(AdapterEventStream {
        receiver,
        cancellation: stream_cancellation,
        task: Some(task),
    })
}

struct AdapterEventStream {
    receiver: mpsc::Receiver<Result<ModelEvent, ModelError>>,
    cancellation: CancellationToken,
    task: Option<tokio::task::JoinHandle<()>>,
}

impl Stream for AdapterEventStream {
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
                            "model adapter stream task failed: {error}"
                        )))))
                    }
                }
            }
        }
    }
}

impl Drop for AdapterEventStream {
    fn drop(&mut self) {
        self.cancellation.cancel();
    }
}

async fn run_stream(
    config: ModelBridgeConfig,
    max_output_tokens: NonZeroU32,
    input: Vec<u8>,
    cancellation: CancellationToken,
    sender: mpsc::Sender<Result<ModelEvent, ModelError>>,
) -> Result<(), ModelError> {
    let mut child = config
        .command("stream", Some(max_output_tokens))
        .spawn()
        .map_err(|error| model_error("start model adapter", error))?;
    let pid = child_pid_raw(&child)
        .map_err(|error| ModelError::new(format!("model adapter ownership failed: {error}")))?;
    let mut stdin = child
        .stdin
        .take()
        .ok_or_else(|| ModelError::new("model adapter stdin is unavailable"))?;
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
        Ok(ReadExit::Cancelled {
            may_have_dispatched,
        }) => {
            stop_bridge_child(&mut child, pid).await?;
            drain_after_stop(writer, stderr).await;
            let error = ModelError::cancelled("model request was cancelled");
            return Err(if may_have_dispatched {
                error.with_unknown_outcome()
            } else {
                error
            });
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
    // Publish completed and error records before touching the child. Cleanup
    // remains owned by this task, but cannot delay or replace the terminal
    // record already available to the consumer.
    publish_terminal(&sender, *terminal).await;
    drop(sender);
    let _ = stop_bridge_child(&mut child, pid).await;
    drain_after_stop(writer, stderr).await;
    Ok(())
}

async fn publish_terminal(
    sender: &mpsc::Sender<Result<ModelEvent, ModelError>>,
    terminal: Result<ModelResponse, ModelError>,
) {
    match terminal {
        Ok(response) => {
            let _ = sender.send(Ok(ModelEvent::Completed { response })).await;
        }
        Err(error) => {
            let _ = sender.send(Err(error)).await;
        }
    }
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
                return Ok(if let Some(terminal) = state.terminal.take() {
                    ReadExit::Finished(terminal)
                } else {
                    ReadExit::Cancelled {
                        may_have_dispatched: state.may_have_dispatched(),
                    }
                });
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
            line = lines.next_line() => line.map_err(|error| model_error("read model adapter stream", error))?,
        };
        let Some(line) = line else {
            break;
        };
        if let Some(event) = state.accept_line(line.as_bytes())?
            && let Some(exit) = forward_delta(sender, cancellation, event).await
        {
            return Ok(exit);
        }
        // A terminal completed/error record is authoritative. Do not keep
        // waiting for EOF — a hung adapter would otherwise sit until idle
        // timeout and replace the valid terminal with a timeout error.
        if state.terminal.is_some() {
            return state.finish();
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
    provider_traffic: bool,
    terminal: Option<Box<Result<ModelResponse, ModelError>>>,
}

impl StreamReadState {
    fn new(deadlines: StreamDeadlines) -> Self {
        Self {
            deadlines,
            idle_deadline: tokio::time::Instant::now() + deadlines.idle,
            received_bytes: 0,
            first_output_seen: false,
            provider_traffic: false,
            terminal: None,
        }
    }

    fn accept_line(&mut self, encoded: &[u8]) -> Result<Option<ModelEvent>, ModelError> {
        self.received_bytes = self
            .received_bytes
            .checked_add(encoded.len())
            .and_then(|bytes| bytes.checked_add(1))
            .ok_or_else(|| ModelError::new("model adapter response size overflowed usize"))?;
        if self.received_bytes > OUTPUT_LIMIT {
            return Err(ModelError::new("model adapter response exceeded 16 MiB"));
        }
        if self.terminal.is_some() {
            return Err(ModelError::new(
                "model adapter emitted data after its terminal record",
            ));
        }
        self.idle_deadline = tokio::time::Instant::now() + self.deadlines.idle;
        Ok(match decode_record(encoded)? {
            StreamRecord::ProviderRequest { payload } => {
                self.provider_traffic = true;
                Some(ModelEvent::ProviderRequest { payload })
            }
            StreamRecord::ProviderResponse { status, headers } => {
                self.provider_traffic = true;
                Some(ModelEvent::ProviderResponse { status, headers })
            }
            StreamRecord::RetryAttempt {
                attempt,
                next_attempt,
                category,
                delay_ms,
                cause_code,
            } => {
                self.provider_traffic = true;
                Some(ModelEvent::RetryAttempt {
                    attempt,
                    next_attempt,
                    category,
                    delay_ms,
                    cause_code,
                })
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
            StreamRecord::Error {
                error,
                error_kind,
                inference_outcome,
                diagnostic,
            } => {
                self.first_output_seen = true;
                self.terminal = Some(Box::new(Err(classified_error(
                    error,
                    error_kind,
                    inference_outcome,
                    diagnostic,
                ))));
                None
            }
        })
    }

    const fn may_have_dispatched(&self) -> bool {
        self.provider_traffic || self.first_output_seen
    }

    fn finish(self) -> Result<ReadExit, ModelError> {
        let terminal = self.terminal.ok_or_else(|| {
            ModelError::new("model adapter closed without a terminal stream record")
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
        () = cancellation.cancelled() => Some(ReadExit::Cancelled {
            may_have_dispatched: true,
        }),
        () = sender.closed() => Some(ReadExit::ReceiverClosed),
        result = sender.send(Ok(event)) => result.err().map(|_| ReadExit::ReceiverClosed),
    }
}

async fn drain_after_stop(
    writer: tokio::task::JoinHandle<std::io::Result<()>>,
    stderr: tokio::task::JoinHandle<std::io::Result<crate::model_bridge::CapturedOutput>>,
) {
    let _ = writer.await;
    let _ = join_output(stderr, "stderr").await;
}

#[derive(Debug)]
enum ReadExit {
    Finished(Box<Result<ModelResponse, ModelError>>),
    Cancelled { may_have_dispatched: bool },
    ReceiverClosed,
}

fn decode_record(encoded: &[u8]) -> Result<StreamRecord, ModelError> {
    let value: serde_json::Value = serde_json::from_slice(encoded)
        .map_err(|error| model_error("decode model adapter stream record", error))?;
    if value.get("event").is_some() {
        serde_json::from_value(value)
            .map_err(|error| model_error("decode model adapter stream record", error))
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
    RetryAttempt {
        attempt: u32,
        next_attempt: u32,
        category: ModelErrorKind,
        delay_ms: u64,
        #[serde(default)]
        cause_code: Option<String>,
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
        #[serde(default)]
        error_kind: Option<ModelErrorKind>,
        #[serde(default)]
        inference_outcome: Option<InferenceOutcome>,
        #[serde(default)]
        diagnostic: Option<ModelFailureDiagnostic>,
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

#[cfg(test)]
#[path = "model_stream_tests.rs"]
mod tests;
