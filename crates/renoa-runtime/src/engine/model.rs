use futures_util::StreamExt;
use renoa_core::{
    CapabilitySpec, Message, ModelEvent, ModelRequest, ModelResponse, RunEventKind, RunId,
};
use tokio_util::sync::CancellationToken;

use crate::{AgentEvent, AgentEventSink, events::emit_event};

use super::{Engine, EngineError};

pub(super) struct ModelStepResult {
    pub(super) response: ModelResponse,
    pub(super) message_started: bool,
}

pub(super) fn build_request(
    run_id: RunId,
    round: u32,
    instructions: &str,
    messages: &[Message],
    capabilities: &[CapabilitySpec],
) -> ModelRequest {
    let mut model_messages = Vec::with_capacity(messages.len() + 1);
    model_messages.push(Message::System {
        text: instructions.to_owned(),
    });
    model_messages.extend_from_slice(messages);
    ModelRequest {
        run_id,
        round,
        messages: model_messages,
        capabilities: capabilities.to_vec(),
    }
}

impl Engine {
    pub(super) async fn model_step(
        &self,
        request: ModelRequest,
        cancellation: CancellationToken,
        event_sink: Option<&dyn AgentEventSink>,
    ) -> Result<ModelStepResult, EngineError> {
        let run_id = request.run_id;
        let round = request.round;
        self.store
            .append_events(run_id, vec![RunEventKind::ModelRequested { round }])
            .await?;

        let mut events = self.model.stream(request, cancellation.child_token());
        let mut message_started = false;
        loop {
            let event = tokio::select! {
                biased;
                () = cancellation.cancelled() => {
                    abort_partial_message(event_sink, message_started).await;
                    return Err(EngineError::Cancelled);
                },
                event = events.next() => event,
            };
            match event {
                Some(Ok(ModelEvent::TextDelta { text })) => {
                    if !message_started {
                        emit_event(
                            event_sink,
                            AgentEvent::MessageStart {
                                message: Message::Assistant {
                                    text: String::new(),
                                    capability_calls: Vec::new(),
                                },
                            },
                        )
                        .await;
                        message_started = true;
                    }
                    emit_event(event_sink, AgentEvent::MessageUpdate { text_delta: text }).await;
                }
                Some(Ok(ModelEvent::Completed { response })) => {
                    if let Err(error) = self
                        .store
                        .append_events(
                            run_id,
                            vec![RunEventKind::ModelResponded {
                                round,
                                response: response.clone(),
                            }],
                        )
                        .await
                    {
                        abort_partial_message(event_sink, message_started).await;
                        return Err(error.into());
                    }
                    return Ok(ModelStepResult {
                        response,
                        message_started,
                    });
                }
                Some(Err(error)) => {
                    abort_partial_message(event_sink, message_started).await;
                    return Err(error.into());
                }
                None => {
                    abort_partial_message(event_sink, message_started).await;
                    return Err(renoa_core::ModelError::new(
                        "model stream ended without a completed response",
                    )
                    .into());
                }
            }
        }
    }
}

async fn abort_partial_message(event_sink: Option<&dyn AgentEventSink>, message_started: bool) {
    if message_started {
        emit_event(event_sink, AgentEvent::MessageAbort).await;
    }
}
