use futures_util::StreamExt;
use thiserror::Error;
use tokio_util::sync::CancellationToken;

use crate::{
    AgentEvent, AgentEventSink, InferenceOutcome, MessageRole, Model, ModelError, ModelErrorKind,
    ModelEvent, ModelEventStream, ModelFailureCode, ModelFailureDiagnostic, ModelRequest,
    ModelResponse, events::emit_event,
};

struct SamplingObserver<'a> {
    invocation_id: String,
    sink: Option<&'a dyn AgentEventSink>,
    message_started: bool,
    provider_traffic: bool,
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
            provider_traffic: false,
        }
    }

    async fn record(&mut self, event: ModelEvent) -> Option<ModelResponse> {
        match event {
            ModelEvent::ProviderRequest { payload } => {
                self.provider_traffic = true;
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
                self.provider_traffic = true;
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
            ModelEvent::RetryAttempt {
                attempt,
                next_attempt,
                category,
                delay_ms,
                cause_code,
            } => {
                self.provider_traffic = true;
                emit_event(
                    self.sink,
                    AgentEvent::ModelRetryAttempt {
                        invocation_id: self.invocation_id.clone(),
                        attempt,
                        next_attempt,
                        category,
                        delay_ms,
                        cause_code,
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

    async fn cancel(&self, may_have_dispatched: bool) {
        self.abort_message().await;
        self.failure(
            ModelFailureCode::Cancelled,
            "model sampling was cancelled".to_owned(),
            may_have_dispatched,
            None,
        )
        .await;
    }

    async fn model_failure(&self, mut error: ModelError) -> ModelError {
        if self.message_started {
            self.abort_message().await;
            if error.inference_outcome() == InferenceOutcome::KnownNotStarted {
                error = error.with_unknown_outcome();
            }
        }
        let outcome_unknown = error.inference_outcome() == InferenceOutcome::Unknown;
        self.failure(
            failure_code(error.kind()),
            error.to_string(),
            outcome_unknown,
            error.diagnostic().cloned(),
        )
        .await;
        error
    }

    async fn incomplete(&self) {
        self.abort_message().await;
        self.failure(
            ModelFailureCode::IncompleteStream,
            "model stream ended without a completed response".to_owned(),
            true,
            None,
        )
        .await;
    }

    async fn abort_message(&self) {
        if self.message_started {
            emit_event(self.sink, AgentEvent::MessageAbort).await;
        }
    }

    async fn failure(
        &self,
        code: ModelFailureCode,
        message: String,
        outcome_unknown: bool,
        diagnostic: Option<ModelFailureDiagnostic>,
    ) {
        emit_event(
            self.sink,
            AgentEvent::ModelRequestFailed {
                invocation_id: self.invocation_id.clone(),
                code,
                message,
                outcome_unknown,
                diagnostic,
            },
        )
        .await;
    }
}

const fn failure_code(kind: ModelErrorKind) -> ModelFailureCode {
    match kind {
        ModelErrorKind::Authentication => ModelFailureCode::Authentication,
        ModelErrorKind::RateLimited => ModelFailureCode::RateLimited,
        ModelErrorKind::InvalidRequest => ModelFailureCode::InvalidRequest,
        ModelErrorKind::ContextWindowExceeded => ModelFailureCode::ContextWindowExceeded,
        ModelErrorKind::Network => ModelFailureCode::Network,
        ModelErrorKind::Timeout => ModelFailureCode::Timeout,
        ModelErrorKind::ProviderUnavailable => ModelFailureCode::ProviderUnavailable,
        ModelErrorKind::Protocol => ModelFailureCode::Protocol,
        ModelErrorKind::StreamInterrupted => ModelFailureCode::StreamInterrupted,
        ModelErrorKind::Cancelled => ModelFailureCode::Cancelled,
        ModelErrorKind::Unknown => ModelFailureCode::Unknown,
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
                return drain_cancelled_stream(
                    &mut stream,
                    &mut observer,
                )
                .await;
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
                if cancellation.is_cancelled()
                    && !observer.provider_traffic
                    && !observer.message_started
                {
                    observer.cancel(false).await;
                    return Err(SamplingError::Cancelled);
                }
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

async fn drain_cancelled_stream(
    stream: &mut ModelEventStream<'_>,
    observer: &mut SamplingObserver<'_>,
) -> Result<SamplingResult, SamplingError> {
    let mut adapter_error = None;
    let mut completed = None;
    while let Some(event) = stream.next().await {
        match event {
            Ok(event) => {
                if let Some(response) = observer.record(event).await {
                    completed = Some(response);
                }
            }
            Err(error) => adapter_error = Some(error),
        }
    }
    if let Some(response) = completed {
        return Ok(SamplingResult {
            response,
            message_started: observer.message_started,
        });
    }
    let may_have_dispatched = observer.provider_traffic || observer.message_started;
    if may_have_dispatched {
        if let Some(error) = adapter_error {
            let error = observer.model_failure(error).await;
            return Err(SamplingError::Model(error));
        }
        observer.cancel(true).await;
        return Err(SamplingError::Model(
            ModelError::cancelled("model sampling was cancelled").with_unknown_outcome(),
        ));
    }
    observer.cancel(false).await;
    Err(SamplingError::Cancelled)
}
