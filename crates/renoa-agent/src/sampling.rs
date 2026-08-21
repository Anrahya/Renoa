use futures_util::StreamExt;
use thiserror::Error;
use tokio_util::sync::CancellationToken;

use crate::{
    AgentEvent, AgentEventSink, MessageRole, Model, ModelError, ModelErrorKind, ModelEvent,
    ModelFailureCode, ModelRequest, ModelResponse, events::emit_event,
};

struct SamplingObserver<'a> {
    invocation_id: String,
    sink: Option<&'a dyn AgentEventSink>,
    message_started: bool,
}

impl<'a> SamplingObserver<'a> {
    async fn start(request: &ModelRequest, sink: Option<&'a dyn AgentEventSink>) -> Self {
        let invocation_id = uuid::Uuid::new_v4().to_string();
        emit_event(
            sink,
            AgentEvent::ModelRequestStart {
                invocation_id: invocation_id.clone(),
                request: request.clone(),
            },
        )
        .await;
        Self {
            invocation_id,
            sink,
            message_started: false,
        }
    }

    async fn record(&mut self, event: ModelEvent) -> Option<ModelResponse> {
        match event {
            ModelEvent::ProviderRequest { payload } => {
                emit_event(
                    self.sink,
                    AgentEvent::ModelProviderRequest {
                        invocation_id: self.invocation_id.clone(),
                        payload,
                    },
                )
                .await;
            }
            ModelEvent::ProviderResponse { status, headers } => {
                emit_event(
                    self.sink,
                    AgentEvent::ModelProviderResponse {
                        invocation_id: self.invocation_id.clone(),
                        status,
                        headers,
                    },
                )
                .await;
            }
            ModelEvent::ContentDelta {
                content_index,
                delta,
            } => {
                emit_event(
                    self.sink,
                    AgentEvent::ModelRequestChunk {
                        invocation_id: self.invocation_id.clone(),
                        content_index,
                        delta: delta.clone(),
                    },
                )
                .await;
                if !self.message_started {
                    emit_event(
                        self.sink,
                        AgentEvent::MessageStart {
                            role: MessageRole::Assistant,
                        },
                    )
                    .await;
                    self.message_started = true;
                }
                emit_event(
                    self.sink,
                    AgentEvent::MessageUpdate {
                        content_index,
                        delta,
                    },
                )
                .await;
            }
            ModelEvent::Completed { response } => {
                emit_event(
                    self.sink,
                    AgentEvent::ModelRequestEnd {
                        invocation_id: self.invocation_id.clone(),
                        response: response.clone(),
                    },
                )
                .await;
                return Some(response);
            }
        }
        None
    }

    async fn cancel(&self) {
        self.abort_message().await;
        self.failure(
            ModelFailureCode::Cancelled,
            "model sampling was cancelled".to_owned(),
            true,
        )
        .await;
    }

    async fn model_failure(&self, mut error: ModelError) -> ModelError {
        if self.message_started {
            self.abort_message().await;
            if error.kind().is_known_before_inference() {
                error = ModelError::new(error.to_string());
            }
        }
        let code = match error.kind() {
            ModelErrorKind::ContextWindowExceeded => ModelFailureCode::ContextWindowExceeded,
            ModelErrorKind::AuthenticationFailed => ModelFailureCode::AuthenticationFailed,
            ModelErrorKind::Timeout => ModelFailureCode::Timeout,
            ModelErrorKind::OutcomeUnknown => ModelFailureCode::OutcomeUnknown,
        };
        let outcome_unknown = !error.kind().is_known_before_inference();
        self.failure(code, error.to_string(), outcome_unknown).await;
        error
    }

    async fn incomplete(&self) {
        self.abort_message().await;
        self.failure(
            ModelFailureCode::IncompleteStream,
            "model stream ended without a completed response".to_owned(),
            true,
        )
        .await;
    }

    async fn abort_message(&self) {
        if self.message_started {
            emit_event(self.sink, AgentEvent::MessageAbort).await;
        }
    }

    async fn failure(&self, code: ModelFailureCode, message: String, outcome_unknown: bool) {
        emit_event(
            self.sink,
            AgentEvent::ModelRequestFailed {
                invocation_id: self.invocation_id.clone(),
                code,
                message,
                outcome_unknown,
            },
        )
        .await;
    }
}

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
    let mut observer = SamplingObserver::start(&request, sink).await;
    let invocation_cancellation = cancellation.child_token();
    let mut stream = model.stream(request, invocation_cancellation.clone());
    loop {
        let event = tokio::select! {
            biased;
            () = cancellation.cancelled() => {
                invocation_cancellation.cancel();
                while stream.next().await.is_some() {}
                observer.cancel().await;
                return Err(SamplingError::Cancelled);
            },
            event = stream.next() => event,
        };
        match event {
            Some(Ok(event)) => {
                let Some(response) = observer.record(event).await else {
                    continue;
                };
                return Ok(SamplingResult {
                    response,
                    message_started: observer.message_started,
                });
            }
            Some(Err(error)) => {
                let error = observer.model_failure(error).await;
                return Err(SamplingError::Model(error));
            }
            None => {
                observer.incomplete().await;
                return Err(SamplingError::IncompleteStream);
            }
        }
    }
}
