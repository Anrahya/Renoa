use std::num::NonZeroU32;

use futures_util::{StreamExt as _, stream};
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
};

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
    tokio::spawn({
        let errors = sender.clone();
        async move {
            if let Err(error) =
                run_stream(config, max_output_tokens, input, cancellation, sender).await
            {
                let _ = errors.send(Err(error)).await;
            }
        }
    });
    stream::unfold(receiver, |mut receiver| async move {
        receiver.recv().await.map(|event| (event, receiver))
    })
    .boxed()
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
    let mut stdin = child
        .stdin
        .take()
        .ok_or_else(|| ModelError::new("Pi model bridge stdin is unavailable"))?;
    let writer = tokio::spawn(async move { stdin.write_all(&input).await });
    let stdout = child.stdout.take().expect("piped stdout");
    let stderr = drain(child.stderr.take().expect("piped stderr"));
    let read = read_records(stdout, &cancellation, &sender).await;
    let terminal = match read {
        Ok(ReadExit::Finished(terminal)) => terminal,
        Ok(ReadExit::Cancelled) => {
            stop_child(&mut child).await?;
            drain_after_stop(writer, stderr).await;
            return Err(ModelError::new("Pi model request was cancelled"));
        }
        Ok(ReadExit::ReceiverClosed) => {
            stop_child(&mut child).await?;
            drain_after_stop(writer, stderr).await;
            return Ok(());
        }
        Err(error) => {
            stop_child(&mut child).await?;
            drain_after_stop(writer, stderr).await;
            return Err(error);
        }
    };
    let status = tokio::select! {
        biased;
        () = cancellation.cancelled() => {
            stop_child(&mut child).await?;
            drain_after_stop(writer, stderr).await;
            return Err(ModelError::new("Pi model request was cancelled"));
        }
        () = sender.closed() => {
            stop_child(&mut child).await?;
            drain_after_stop(writer, stderr).await;
            return Ok(());
        }
        status = child.wait() => {
            status.map_err(|error| model_error("wait for Pi model bridge", error))?
        }
    };
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
    stdout: tokio::process::ChildStdout,
    cancellation: &CancellationToken,
    sender: &mpsc::Sender<Result<ModelEvent, ModelError>>,
) -> Result<ReadExit, ModelError> {
    let output_limit = u64::try_from(OUTPUT_LIMIT + 1).expect("16 MiB fits in u64");
    let mut lines = BufReader::new(stdout.take(output_limit)).lines();
    let mut received_bytes = 0_usize;
    let mut terminal = None;
    loop {
        let line = tokio::select! {
            biased;
            () = cancellation.cancelled() => {
                return Ok(ReadExit::Cancelled);
            }
            () = sender.closed() => {
                return Ok(ReadExit::ReceiverClosed);
            }
            line = lines.next_line() => line.map_err(|error| model_error("read Pi model stream", error))?,
        };
        let Some(line) = line else {
            break;
        };
        received_bytes = received_bytes
            .checked_add(line.len() + 1)
            .ok_or_else(|| ModelError::new("Pi model response size overflowed usize"))?;
        if received_bytes > OUTPUT_LIMIT {
            return Err(ModelError::new("Pi model response exceeded 16 MiB"));
        }
        if terminal.is_some() {
            return Err(ModelError::new(
                "Pi model bridge emitted data after its terminal record",
            ));
        }
        match decode_record(line.as_bytes())? {
            StreamRecord::ContentDelta {
                content_index,
                delta,
            } => {
                if let Some(exit) = forward_delta(
                    sender,
                    cancellation,
                    ModelEvent::ContentDelta {
                        content_index,
                        delta: delta.into(),
                    },
                )
                .await
                {
                    return Ok(exit);
                }
            }
            StreamRecord::Completed { response } => terminal = Some(Box::new(Ok(response))),
            StreamRecord::Error { error, error_kind } => {
                terminal = Some(Box::new(Err(match error_kind {
                    Some(BridgeErrorKind::ContextWindowExceeded) => {
                        ModelError::context_window_exceeded(error)
                    }
                    None => ModelError::new(error),
                })));
            }
        }
    }
    let terminal = terminal.ok_or_else(|| {
        ModelError::new("Pi model bridge closed without a terminal stream record")
    })?;
    Ok(ReadExit::Finished(terminal))
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

async fn stop_child(child: &mut tokio::process::Child) -> Result<(), ModelError> {
    let _ = child.start_kill();
    child
        .wait()
        .await
        .map_err(|error| model_error("reap Pi model bridge", error))?;
    Ok(())
}

async fn drain_after_stop(
    writer: tokio::task::JoinHandle<std::io::Result<()>>,
    stderr: tokio::task::JoinHandle<std::io::Result<crate::pi_model::CapturedOutput>>,
) {
    let _ = writer.await;
    let _ = join_output(stderr, "stderr").await;
}

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
}
