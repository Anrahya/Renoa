use std::sync::Arc;

use renoa_agent::{
    AgentEvent, AgentEventSink, Model, ModelErrorKind, ModelRequest, SamplingError, Tool, ToolCall,
    invoke_tool, sample_model,
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
    pub(crate) const fn new(
        model: Arc<dyn Model>,
        events: Option<Arc<dyn AgentEventSink>>,
    ) -> Self {
        Self { model, events }
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
                    if error.kind() == ModelErrorKind::ContextWindowExceeded =>
                {
                    model_completion(ModelEffectOutput::ContextWindowExceeded {
                        message: error.to_string(),
                    })
                }
                Err(SamplingError::Model(error))
                    if error.kind() == ModelErrorKind::AuthenticationFailed =>
                {
                    failure("model authentication failed", error)
                }
                // Every other current or future sampling failure is uncertain
                // until its semantics are explicitly classified as pre-dispatch.
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
