use futures_util::StreamExt;
use renoa_core::{
    CapabilitySpec, Message, ModelEvent, ModelRequest, ModelResponse, RunEventKind, RunId,
};
use tokio_util::sync::CancellationToken;

use super::{Engine, EngineError};

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
    ) -> Result<ModelResponse, EngineError> {
        let run_id = request.run_id;
        let round = request.round;
        self.store
            .append_events(run_id, vec![RunEventKind::ModelRequested { round }])
            .await?;

        let mut events = self.model.stream(request, cancellation.child_token());
        loop {
            let event = tokio::select! {
                biased;
                () = cancellation.cancelled() => return Err(EngineError::Cancelled),
                event = events.next() => event,
            };
            match event {
                Some(Ok(ModelEvent::TextDelta { .. })) => {}
                Some(Ok(ModelEvent::Completed { response })) => {
                    self.store
                        .append_events(
                            run_id,
                            vec![RunEventKind::ModelResponded {
                                round,
                                response: response.clone(),
                            }],
                        )
                        .await?;
                    return Ok(response);
                }
                Some(Err(error)) => return Err(error.into()),
                None => {
                    return Err(renoa_core::ModelError::new(
                        "model stream ended without a completed response",
                    )
                    .into());
                }
            }
        }
    }
}
