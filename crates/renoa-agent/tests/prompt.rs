use std::sync::{Arc, Mutex};

use futures_util::{StreamExt, stream};
use renoa_agent::{
    Agent, AssistantContent, Message, Model, ModelEvent, ModelEventStream, ModelRequest,
    ModelResponse, StopReason,
};
use tokio_util::sync::CancellationToken;

#[tokio::test]
async fn text_prompt_runs_without_protocol_or_storage_configuration() {
    let model = Arc::new(ScriptedModel::new(ModelResponse {
        content: vec![AssistantContent::text("Hello from Renoa.")],
        stop_reason: StopReason::Stop,
        usage: None,
        metadata: renoa_agent::AssistantMetadata::default(),
    }));
    let mut agent = Agent::new(model.clone(), "You are concise.");

    let result = agent
        .prompt("Hello")
        .await
        .expect("standalone prompt must complete");

    assert_eq!(result.output, "Hello from Renoa.");
    assert_eq!(result.model_turns, 1);
    assert_eq!(
        model.requests(),
        vec![ModelRequest {
            system_prompt: "You are concise.".to_owned(),
            messages: vec![Message::user_text("Hello")],
            tools: Vec::new(),
        }]
    );
}

struct ScriptedModel {
    response: Mutex<Option<ModelResponse>>,
    requests: Mutex<Vec<ModelRequest>>,
}

impl ScriptedModel {
    fn new(response: ModelResponse) -> Self {
        Self {
            response: Mutex::new(Some(response)),
            requests: Mutex::new(Vec::new()),
        }
    }

    fn requests(&self) -> Vec<ModelRequest> {
        self.requests
            .lock()
            .expect("model request lock must not be poisoned")
            .clone()
    }
}

impl Model for ScriptedModel {
    fn stream(
        &self,
        request: ModelRequest,
        _cancellation: CancellationToken,
    ) -> ModelEventStream<'_> {
        self.requests
            .lock()
            .expect("model request lock must not be poisoned")
            .push(request);
        let response = self
            .response
            .lock()
            .expect("model response lock must not be poisoned")
            .take()
            .expect("scripted response must exist");
        stream::once(async { Ok(ModelEvent::Completed { response }) }).boxed()
    }
}
