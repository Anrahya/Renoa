use std::{
    collections::VecDeque,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
};

use futures_util::{StreamExt, stream};
use renoa_agent::{
    Agent, AgentEvent, AgentEventSink, AgentHandle, AssistantContent, BoxFuture, ContentBlock,
    Message, Model, ModelEvent, ModelEventStream, ModelRequest, ModelResponse, StopReason,
};
use tokio_util::sync::CancellationToken;

#[tokio::test]
async fn structured_queued_input_reaches_model_requests_unchanged() {
    let model = Arc::new(RecordingModel::new(vec![
        text_response("Send both."),
        text_response("Steering received."),
        text_response("Follow-up received."),
    ]));
    let agent = Agent::new(model.clone(), "Inspect the supplied content.");
    let steering = vec![
        ContentBlock::text("Here is the screenshot."),
        ContentBlock::image("base64-image", "image/png"),
    ];
    let follow_up = vec![ContentBlock::text("Now summarize it.")];
    let sink = Arc::new(QueueAfterFirstAssistant::new(
        agent.handle(),
        steering.clone(),
        follow_up.clone(),
    ));
    let mut agent = agent.with_event_sink(sink);

    let result = agent
        .prompt("What do you need?")
        .await
        .expect("structured queued input must complete");

    assert_eq!(result.output, "Follow-up received.");
    assert_eq!(
        model.requests()[1].messages.last(),
        Some(&Message::User { content: steering })
    );
    assert_eq!(
        model.requests()[2].messages.last(),
        Some(&Message::User { content: follow_up })
    );
}

struct RecordingModel {
    responses: Mutex<VecDeque<ModelResponse>>,
    requests: Mutex<Vec<ModelRequest>>,
}

impl RecordingModel {
    fn new(responses: Vec<ModelResponse>) -> Self {
        Self {
            responses: Mutex::new(responses.into()),
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
        let response = self
            .responses
            .lock()
            .expect("model response lock must not be poisoned")
            .pop_front()
            .expect("scripted response must exist");
        stream::once(async { Ok(ModelEvent::Completed { response }) }).boxed()
    }
}

struct QueueAfterFirstAssistant {
    handle: AgentHandle,
    steering: Vec<ContentBlock>,
    follow_up: Vec<ContentBlock>,
    queued: AtomicBool,
}

impl QueueAfterFirstAssistant {
    fn new(handle: AgentHandle, steering: Vec<ContentBlock>, follow_up: Vec<ContentBlock>) -> Self {
        Self {
            handle,
            steering,
            follow_up,
            queued: AtomicBool::new(false),
        }
    }
}

impl AgentEventSink for QueueAfterFirstAssistant {
    fn emit(&self, event: AgentEvent) -> BoxFuture<'_, ()> {
        Box::pin(async move {
            if matches!(
                event,
                AgentEvent::MessageEnd {
                    message: Message::Assistant { .. }
                }
            ) && !self.queued.swap(true, Ordering::SeqCst)
            {
                self.handle
                    .steer_content(self.steering.clone())
                    .expect("steering queue must accept structured content");
                self.handle
                    .follow_up_content(self.follow_up.clone())
                    .expect("follow-up queue must accept structured content");
            }
        })
    }
}

fn text_response(text: &str) -> ModelResponse {
    ModelResponse {
        content: vec![AssistantContent::text(text)],
        stop_reason: StopReason::Stop,
        usage: None,
        metadata: renoa_agent::AssistantMetadata::default(),
    }
}
