use futures_util::StreamExt;
use thiserror::Error;
use tokio_util::sync::CancellationToken;

use crate::{
    AgentEvent, AgentEventSink, MessageRole, Model, ModelError, ModelEvent, ModelRequest,
    ModelResponse, events::emit_event,
};

/// The complete result of one model-adapter invocation.
#[non_exhaustive]
pub struct SamplingResult {
    pub response: ModelResponse,
    pub(crate) message_started: bool,
}

#[derive(Debug, Error)]
#[non_exhaustive]
pub enum SamplingError {
    #[error("model sampling was cancelled")]
    Cancelled,
    #[error("model invocation failed: {0}")]
    Model(#[from] ModelError),
    #[error("model stream ended without a completed response")]
    IncompleteStream,
}

/// Drives one exact provider-neutral request to a complete response.
///
/// Transient deltas are sent to `sink`; this function does not mutate a
/// conversation or decide what a completed response means.
///
/// # Errors
///
/// Returns cancellation, provider, and incomplete-stream failures distinctly.
pub async fn sample_model(
    model: &dyn Model,
    request: ModelRequest,
    cancellation: CancellationToken,
    sink: Option<&dyn AgentEventSink>,
) -> Result<SamplingResult, SamplingError> {
    if cancellation.is_cancelled() {
        return Err(SamplingError::Cancelled);
    }
    let mut stream = model.stream(request, cancellation.child_token());
    let mut message_started = false;
    loop {
        let event = tokio::select! {
            biased;
            () = cancellation.cancelled() => {
                if message_started {
                    emit_event(sink, AgentEvent::MessageAbort).await;
                }
                return Err(SamplingError::Cancelled);
            },
            event = stream.next() => event,
        };
        match event {
            Some(Ok(ModelEvent::ContentDelta {
                content_index,
                delta,
            })) => {
                if !message_started {
                    emit_event(
                        sink,
                        AgentEvent::MessageStart {
                            role: MessageRole::Assistant,
                        },
                    )
                    .await;
                    message_started = true;
                }
                emit_event(
                    sink,
                    AgentEvent::MessageUpdate {
                        content_index,
                        delta,
                    },
                )
                .await;
            }
            Some(Ok(ModelEvent::Completed { response })) => {
                return Ok(SamplingResult {
                    response,
                    message_started,
                });
            }
            Some(Err(error)) => {
                if message_started {
                    emit_event(sink, AgentEvent::MessageAbort).await;
                }
                return Err(SamplingError::Model(error));
            }
            None => {
                if message_started {
                    emit_event(sink, AgentEvent::MessageAbort).await;
                }
                return Err(SamplingError::IncompleteStream);
            }
        }
    }
}
