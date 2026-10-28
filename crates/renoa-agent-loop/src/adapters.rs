use std::sync::Arc;

use renoa_agent::{
    AgentEvent, AgentEventSink, BoxFuture, InferenceOutcome, Model, ModelErrorKind, ModelRequest,
    SamplingError, Tool, ToolCall, invoke_tool, sample_model,
};
use renoa_kernel::{
    EffectAdapter, EffectCompletion, EffectFuture, EffectInvocation, EffectOutcome,
};

use crate::format::ModelEffectOutput;

pub(crate) struct ModelAdapter {
    model: Arc<dyn Model>,
    events: Option<Arc<dyn AgentEventSink>>,
}

impl ModelAdapter {
    /// The adapter of a conversational model request.
    pub(crate) const fn reply(
        model: Arc<dyn Model>,
        events: Option<Arc<dyn AgentEventSink>>,
    ) -> Self {
        Self { model, events }
    }

    /// The adapter of a context-summary request. Observers see the request,
    /// its retries, failure and usage, but never an assistant message, because
    /// a summary is not a reply.
    pub(crate) fn summary(model: Arc<dyn Model>, events: Option<Arc<dyn AgentEventSink>>) -> Self {
        Self {
            model,
            events: events.map(|inner| Arc::new(SummaryEvents(inner)) as Arc<dyn AgentEventSink>),
        }
    }
}

/// Forwards a summary request's events except the ones that describe an
/// assistant message. The match is exhaustive so a new event kind must be
/// classified here before it compiles.
struct SummaryEvents(Arc<dyn AgentEventSink>);

impl AgentEventSink for SummaryEvents {
    fn emit(&self, event: AgentEvent) -> BoxFuture<'_, ()> {
        match event {
            AgentEvent::MessageStart { .. }
            | AgentEvent::MessageUpdate { .. }
            | AgentEvent::MessageAbort => Box::pin(std::future::ready(())),
            event @ (AgentEvent::ModelRequestStart { .. }
            | AgentEvent::ModelProviderRequest { .. }
            | AgentEvent::ModelProviderResponse { .. }
            | AgentEvent::ModelRequestChunk { .. }
            | AgentEvent::ModelRequestEnd { .. }
            | AgentEvent::ModelRequestFailed { .. }
            | AgentEvent::ModelRetryAttempt { .. }
            | AgentEvent::ToolExecutionStart { .. }
            | AgentEvent::ToolExecutionUpdate { .. }
            | AgentEvent::ToolExecutionEnd { .. }
            | AgentEvent::ToolExecutionOutcomeUnknown { .. }) => self.0.emit(event),
        }
    }
}

impl EffectAdapter for ModelAdapter {
    fn invoke(&self, invocation: EffectInvocation) -> EffectFuture<'_> {
        let request = serde_json::from_value::<ModelRequest>(invocation.request);
        let cancellation = invocation.cancellation;
        let model = Arc::clone(&self.model);
        let events = self.events.as_ref().map(Arc::clone);
        Box::pin(async move {
            let request = match request {
                Ok(request) => request,
                Err(error) => return failure("invalid persisted model request", error),
            };
            match sample_model(model.as_ref(), request, cancellation, events.as_deref()).await {
                Ok(result) => model_completion(ModelEffectOutput::Completed {
                    response: result.response,
                }),
                Err(SamplingError::Model(error))
                    if error.kind() == ModelErrorKind::ContextWindowExceeded
                        && error.inference_outcome() == InferenceOutcome::KnownNotStarted =>
                {
                    model_completion(ModelEffectOutput::ContextWindowExceeded {
                        message: error.to_string(),
                    })
                }
                Err(SamplingError::Cancelled) => EffectOutcome::Failure {
                    message: "model sampling was cancelled".to_owned(),
                }
                .into(),
                Err(SamplingError::Model(error))
                    if error.inference_outcome() == InferenceOutcome::KnownNotStarted =>
                {
                    EffectOutcome::Failure {
                        message: error.to_string(),
                    }
                    .into()
                }
                Err(_) => EffectCompletion::OutcomeUnknown,
            }
        })
    }
}

fn model_completion(output: ModelEffectOutput) -> EffectCompletion {
    match serde_json::to_value(output) {
        Ok(output) => EffectOutcome::Success(output).into(),
        Err(error) => failure("model response serialization failed", error),
    }
}

pub(crate) struct ToolAdapter {
    tool: Arc<dyn Tool>,
    events: Option<Arc<dyn AgentEventSink>>,
}

impl ToolAdapter {
    pub(crate) const fn new(tool: Arc<dyn Tool>, events: Option<Arc<dyn AgentEventSink>>) -> Self {
        Self { tool, events }
    }
}

impl EffectAdapter for ToolAdapter {
    fn invoke(&self, invocation: EffectInvocation) -> EffectFuture<'_> {
        let call = serde_json::from_value::<ToolCall>(invocation.request);
        let cancellation = invocation.cancellation;
        let tool = Arc::clone(&self.tool);
        let events = self.events.as_ref().map(Arc::clone);
        Box::pin(async move {
            let call = match call {
                Ok(call) => call,
                Err(error) => return failure("invalid persisted tool request", error),
            };
            emit(
                events.as_deref(),
                AgentEvent::ToolExecutionStart { call: call.clone() },
            )
            .await;
            match invoke_tool(
                Some(tool.as_ref()),
                call.clone(),
                cancellation,
                events.as_deref(),
            )
            .await
            {
                Ok(result) => match serde_json::to_value(&result) {
                    Ok(encoded) => {
                        emit(
                            events.as_deref(),
                            AgentEvent::ToolExecutionEnd { call, result },
                        )
                        .await;
                        EffectOutcome::Success(encoded).into()
                    }
                    Err(error) => failure("tool result serialization failed", error),
                },
                Err(error) => {
                    emit(
                        events.as_deref(),
                        AgentEvent::ToolExecutionOutcomeUnknown { call, error },
                    )
                    .await;
                    EffectCompletion::OutcomeUnknown
                }
            }
        })
    }
}

async fn emit(events: Option<&dyn AgentEventSink>, event: AgentEvent) {
    if let Some(events) = events {
        events.emit(event).await;
    }
}

fn failure(action: &str, error: impl std::fmt::Display) -> EffectCompletion {
    EffectOutcome::Failure {
        message: format!("{action}: {error}"),
    }
    .into()
}
