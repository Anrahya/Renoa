use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, Ordering},
};

use futures_util::{StreamExt, stream};
use renoa_agent::{
    Agent, AgentEvent, AgentEventSink, AgentHandle, AssistantContent, BoxFuture, ContextProjector,
    Message, Model, ModelEvent, ModelEventStream, ModelRequest, ModelResponse, StopReason,
};
use tokio_util::sync::CancellationToken;

#[tokio::test]
async fn host_projection_runs_before_each_model_request_without_rewriting_state() {
    let model = Arc::new(RecordingModel::new(["first", "second"]));
    let projector = Arc::new(RecordingProjector::default());
    let agent =
        Agent::new(model.clone(), "Host instructions.").with_context_projector(projector.clone());
    let handle = agent.handle();
    let mut agent = agent.with_event_sink(Arc::new(QueueFollowUp {
        handle,
        queued: AtomicBool::new(false),
    }));

    agent.prompt("original").await.expect("run must complete");

    assert_eq!(
        projector.inputs(),
        vec![
            vec![Message::user_text("original")],
            vec![
                Message::user_text("original"),
                assistant("first"),
                Message::user_text("follow-up"),
            ],
        ]
    );
    assert_eq!(
        model
            .requests()
            .into_iter()
            .map(|request| request.messages)
            .collect::<Vec<_>>(),
        vec![
            vec![Message::user_text("projection-1")],
            vec![Message::user_text("projection-2")],
        ]
    );
    assert_eq!(
        agent.state().messages(),
        &[
            Message::user_text("original"),
            assistant("first"),
            Message::user_text("follow-up"),
            assistant("second"),
        ]
    );
}

#[derive(Default)]
struct RecordingProjector {
    inputs: Mutex<Vec<Vec<Message>>>,
}

impl RecordingProjector {
    fn inputs(&self) -> Vec<Vec<Message>> {
        self.inputs.lock().expect("projection lock").clone()
    }
}

impl ContextProjector for RecordingProjector {
    fn project(
        &self,
        messages: Vec<Message>,
        _cancellation: CancellationToken,
    ) -> BoxFuture<'_, Result<Vec<Message>, renoa_agent::ContextProjectionError>> {
        let projection_number = {
            let mut inputs = self.inputs.lock().expect("projection lock");
            inputs.push(messages);
            inputs.len()
        };
        Box::pin(async move {
            Ok(vec![Message::user_text(format!(
                "projection-{projection_number}"
            ))])
        })
    }
}

struct QueueFollowUp {
    handle: AgentHandle,
    queued: AtomicBool,
}

impl AgentEventSink for QueueFollowUp {
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
                    .follow_up("follow-up")
                    .expect("follow-up must fit");
            }
        })
    }
}

struct RecordingModel {
    responses: Mutex<std::collections::VecDeque<String>>,
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
        self.requests.lock().expect("request lock").clone()
    }
}

impl Model for RecordingModel {
    fn stream(
        &self,
        request: ModelRequest,
        _cancellation: CancellationToken,
    ) -> ModelEventStream<'_> {
        self.requests.lock().expect("request lock").push(request);
        let text = self
            .responses
            .lock()
            .expect("response lock")
            .pop_front()
            .expect("scripted response");
        stream::once(async move {
            Ok(ModelEvent::Completed {
                response: ModelResponse {
                    content: vec![AssistantContent::text(text)],
                    stop_reason: StopReason::Stop,
                    usage: None,
                    metadata: renoa_agent::AssistantMetadata::default(),
                },
            })
        })
        .boxed()
    }
}

fn assistant(text: &str) -> Message {
    Message::Assistant {
        content: vec![AssistantContent::text(text)],
        stop_reason: StopReason::Stop,
        usage: None,
        metadata: renoa_agent::AssistantMetadata::default(),
    }
}
