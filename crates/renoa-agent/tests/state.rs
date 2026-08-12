use std::{
    collections::VecDeque,
    sync::{Arc, Mutex},
};

use futures_util::{StreamExt, stream};
use renoa_agent::{
    Agent, AgentState, AssistantContent, AssistantMetadata, ContentBlock, Message, Model,
    ModelEvent, ModelEventStream, ModelRequest, ModelResponse, StopReason, ToolCall, ToolResult,
};
use tokio_util::sync::CancellationToken;

#[tokio::test]
async fn host_can_supply_a_projected_active_transcript() {
    let state = AgentState::from_messages(vec![Message::user_text("Projected context.")]);
    let model = Arc::new(RecordingModel::new(["Continued reply."]));
    let mut agent = Agent::from_state(model.clone(), "Host instructions.", state);

    agent
        .resume()
        .await
        .expect("projected user context must be resumable");

    assert_eq!(
        model.requests(),
        vec![ModelRequest {
            system_prompt: "Host instructions.".to_owned(),
            messages: vec![Message::user_text("Projected context.")],
            tools: Vec::new(),
        }]
    );
}

#[tokio::test]
async fn structured_user_content_reaches_state_and_model_unchanged() {
    let content = vec![
        ContentBlock::text("What is in this image?"),
        ContentBlock::image("base64-image", "image/png"),
    ];
    let model = Arc::new(RecordingModel::new(["A diagram."]));
    let mut agent = Agent::new(model.clone(), "Inspect images.");

    agent
        .prompt_content(content.clone())
        .await
        .expect("structured prompt must complete");

    let user = Message::User { content };
    assert_eq!(model.requests()[0].messages, vec![user.clone()]);
    assert_eq!(agent.state().messages().first(), Some(&user));
}

#[tokio::test]
async fn provider_continuity_survives_state_restoration() {
    let messages = vec![
        Message::User {
            content: vec![
                ContentBlock::text("Inspect this image."),
                ContentBlock::image("base64-image", "image/png"),
            ],
        },
        Message::Assistant {
            content: vec![
                AssistantContent::signed_text("I will inspect it.", "text-item-1"),
                AssistantContent::reasoning(
                    "The image contains code.",
                    Some("reasoning-item-1".to_owned()),
                    false,
                ),
                AssistantContent::tool_call(ToolCall {
                    id: "call-1".to_owned(),
                    name: "inspect".to_owned(),
                    arguments: serde_json::json!({ "target": "image" }),
                    thought_signature: Some("thought-1".to_owned()),
                    namespace: Some("workspace".to_owned()),
                }),
            ],
            stop_reason: StopReason::ToolUse,
            usage: None,
            metadata: AssistantMetadata {
                api: Some("responses".to_owned()),
                provider: Some("openai".to_owned()),
                model: Some("gpt-test".to_owned()),
                response_model: Some("gpt-test-2026-08-01".to_owned()),
                response_id: Some("response-1".to_owned()),
                raw_stop_reason: Some("tool_calls".to_owned()),
            },
        },
        Message::Tool {
            result: ToolResult {
                call_id: "call-1".to_owned(),
                name: "inspect".to_owned(),
                content: vec![ContentBlock::text("inspection complete")],
                details: None,
                is_error: false,
            },
        },
    ];
    let encoded = serde_json::to_string(&AgentState::from_messages(messages.clone()))
        .expect("state must serialize");
    let state: AgentState = serde_json::from_str(&encoded).expect("state must deserialize");
    let model = Arc::new(RecordingModel::new(["Done."]));
    let mut agent = Agent::from_state(model.clone(), "Host instructions.", state);

    agent.resume().await.expect("tool-result tail must resume");

    assert_eq!(model.requests()[0].messages, messages);
}

#[tokio::test]
async fn serialized_state_continues_a_conversation_with_local_system_instructions() {
    let first_model = Arc::new(RecordingModel::new(["First reply."]));
    let mut first_agent = Agent::new(first_model, "Original instructions.");
    first_agent
        .prompt("First prompt.")
        .await
        .expect("first prompt must complete");

    let encoded = serde_json::to_string(first_agent.state()).expect("state must serialize");
    assert!(!encoded.contains("Original instructions."));
    let state: AgentState = serde_json::from_str(&encoded).expect("state must deserialize");

    let second_model = Arc::new(RecordingModel::new(["Second reply."]));
    let mut second_agent =
        Agent::from_state(second_model.clone(), "Restored host instructions.", state);
    second_agent
        .prompt("Second prompt.")
        .await
        .expect("continued prompt must complete");

    assert_eq!(
        second_model.requests(),
        vec![ModelRequest {
            system_prompt: "Restored host instructions.".to_owned(),
            messages: vec![
                Message::user_text("First prompt."),
                Message::Assistant {
                    content: vec![AssistantContent::text("First reply.")],
                    stop_reason: StopReason::Stop,
                    usage: None,
                    metadata: AssistantMetadata::default(),
                },
                Message::user_text("Second prompt."),
            ],
            tools: Vec::new(),
        }]
    );
}

#[tokio::test]
async fn reset_clears_conversation_and_queued_input_without_reconfiguring_the_agent() {
    let model = Arc::new(RecordingModel::new(["Old reply.", "Fresh reply."]));
    let mut agent = Agent::new(model.clone(), "Keep these instructions.");
    agent
        .prompt("Old prompt.")
        .await
        .expect("initial prompt must complete");
    let handle = agent.handle();
    handle.steer("stale steering").expect("steering must fit");
    handle
        .follow_up("stale follow-up")
        .expect("follow-up must fit");

    agent.reset();

    assert!(agent.state().messages().is_empty());
    assert!(!handle.has_queued_messages());
    agent
        .prompt("Fresh prompt.")
        .await
        .expect("reset agent must remain usable");
    assert_eq!(
        model.requests()[1],
        ModelRequest {
            system_prompt: "Keep these instructions.".to_owned(),
            messages: vec![Message::user_text("Fresh prompt.")],
            tools: Vec::new(),
        }
    );
}

struct RecordingModel {
    responses: Mutex<VecDeque<String>>,
    requests: Mutex<Vec<ModelRequest>>,
}

impl RecordingModel {
    fn new<const N: usize>(responses: [&str; N]) -> Self {
        Self {
            responses: Mutex::new(responses.into_iter().map(str::to_owned).collect()),
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

impl Model for RecordingModel {
    fn stream(
        &self,
        request: ModelRequest,
        _cancellation: CancellationToken,
    ) -> ModelEventStream<'_> {
        self.requests
            .lock()
            .expect("model request lock must not be poisoned")
            .push(request);
        let text = self
            .responses
            .lock()
            .expect("model response lock must not be poisoned")
            .pop_front()
            .expect("scripted response must exist");
        stream::once(async move {
            Ok(ModelEvent::Completed {
                response: ModelResponse {
                    content: vec![AssistantContent::text(text)],
                    stop_reason: StopReason::Stop,
                    usage: None,
                    metadata: AssistantMetadata::default(),
                },
            })
        })
        .boxed()
    }
}
