use std::sync::{Arc, Mutex};

use futures_util::{StreamExt, stream};
use renoa_agent::{
    Agent, AgentState, AssistantContent, Message, Model, ModelEvent, ModelEventStream,
    ModelRequest, ModelResponse, StopReason, TokenUsage,
};
use tokio_util::sync::CancellationToken;

#[tokio::test]
async fn stop_outcome_and_usage_survive_state_restoration() {
    let usage = TokenUsage {
        input: 12,
        output: 5,
        cache_read: 7,
        cache_write: 2,
    };
    let model = Arc::new(SingleResponseModel::new(ModelResponse {
        content: vec![AssistantContent::text("Done.")],
        stop_reason: StopReason::Stop,
        usage: Some(usage),
        metadata: renoa_agent::AssistantMetadata::default(),
    }));
    let mut agent = Agent::new(model, "Be useful.");

    let result = agent
        .prompt("Finish this")
        .await
        .expect("the prompt must complete");

    assert_eq!(result.stop_reason, StopReason::Stop);
    assert_eq!(result.usage, Some(usage));
    assert_eq!(
        agent.state().messages(),
        [
            Message::user_text("Finish this"),
            Message::Assistant {
                content: vec![AssistantContent::text("Done.")],
                stop_reason: StopReason::Stop,
                usage: Some(usage),
                metadata: renoa_agent::AssistantMetadata::default(),
            },
        ]
    );

    let encoded = serde_json::to_string(agent.state()).expect("state must serialize");
    let restored: AgentState = serde_json::from_str(&encoded).expect("state must restore");
    assert_eq!(&restored, agent.state());
}

#[tokio::test]
async fn missing_provider_usage_is_not_reported_as_zero() {
    let model = Arc::new(SingleResponseModel::new(ModelResponse {
        content: vec![AssistantContent::text("Done.")],
        stop_reason: StopReason::Stop,
        usage: None,
        metadata: renoa_agent::AssistantMetadata::default(),
    }));
    let mut agent = Agent::new(model, "Be honest.");

    let result = agent
        .prompt("Finish this")
        .await
        .expect("the prompt must complete");

    assert_eq!(result.usage, None);
    assert!(matches!(
        agent.state().messages().last(),
        Some(Message::Assistant { usage: None, .. })
    ));
}

struct SingleResponseModel {
    response: Mutex<Option<ModelResponse>>,
}

impl SingleResponseModel {
    fn new(response: ModelResponse) -> Self {
        Self {
            response: Mutex::new(Some(response)),
        }
    }
}

impl Model for SingleResponseModel {
    fn stream(
        &self,
        _request: ModelRequest,
        _cancellation: CancellationToken,
    ) -> ModelEventStream<'_> {
        let response = self
            .response
            .lock()
            .expect("model response lock must not be poisoned")
            .take()
            .expect("scripted response must exist");
        stream::once(async { Ok(ModelEvent::Completed { response }) }).boxed()
    }
}
