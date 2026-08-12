use std::{
    collections::VecDeque,
    sync::{
        Arc, Mutex,
        atomic::{AtomicUsize, Ordering},
    },
};

use futures_util::{StreamExt, stream};
use renoa_agent::{
    Agent, AgentConfig, AgentError, AgentEvent, AgentEventSink, AssistantContent, BoxFuture,
    ContentBlock, Message, Model, ModelError, ModelEvent, ModelEventStream, ModelRequest,
    ModelResponse, StopReason, Tool, ToolCall, ToolError, ToolOutput, ToolResult, ToolSpec,
    ToolUpdates,
};
use serde_json::json;
use tokio::sync::Semaphore;
use tokio_util::sync::CancellationToken;

#[tokio::test]
async fn resume_retries_a_failed_user_tail_without_duplicating_it() {
    let model = Arc::new(ScriptedModel::new(vec![
        Step::Fail("provider disconnected"),
        Step::Respond("Recovered."),
    ]));
    let mut agent = Agent::new(model.clone(), "Be concise.");

    let error = agent
        .prompt("Try once")
        .await
        .expect_err("the first provider request must fail");
    assert!(matches!(error, AgentError::Model(_)));

    let result = agent
        .resume()
        .await
        .expect("resume must retry from existing context");

    assert_eq!(result.output, "Recovered.");
    assert_eq!(result.model_turns, 1);
    let expected_messages = vec![Message::user_text("Try once")];
    assert_eq!(model.requests()[0].messages, expected_messages);
    assert_eq!(model.requests()[1].messages, expected_messages);
}

#[tokio::test]
async fn resume_processes_a_queued_follow_up_after_an_assistant_tail() {
    let model = Arc::new(ScriptedModel::new(vec![
        Step::Respond("Initial answer."),
        Step::Respond("Follow-up answer."),
    ]));
    let mut agent = Agent::new(model.clone(), "Be concise.");

    agent
        .prompt("Initial request")
        .await
        .expect("initial prompt must complete");
    agent
        .handle()
        .follow_up("Queued follow-up")
        .expect("follow-up must fit");

    let result = agent
        .resume()
        .await
        .expect("queued input must make an assistant tail resumable");

    assert_eq!(result.output, "Follow-up answer.");
    assert_eq!(result.model_turns, 1);
    assert_eq!(
        model.requests()[1].messages.last(),
        Some(&Message::user_text("Queued follow-up"))
    );
}

#[tokio::test]
async fn resume_drains_steering_one_message_per_turn() {
    let model = Arc::new(ScriptedModel::new(vec![
        Step::Respond("Initial answer."),
        Step::Respond("First adjustment."),
        Step::Respond("Second adjustment."),
    ]));
    let mut agent = Agent::new(model.clone(), "Be concise.");
    agent
        .prompt("Initial request")
        .await
        .expect("initial prompt must complete");
    let handle = agent.handle();
    handle.steer("First steer").expect("first steer must fit");
    handle.steer("Second steer").expect("second steer must fit");

    let result = agent
        .resume()
        .await
        .expect("queued steering must resume the conversation");

    assert_eq!(result.output, "Second adjustment.");
    assert_eq!(result.model_turns, 2);
    let requests = model.requests();
    assert_eq!(
        requests[1].messages.last(),
        Some(&Message::user_text("First steer"))
    );
    assert_eq!(
        requests[2].messages.last(),
        Some(&Message::user_text("Second steer"))
    );
}

#[tokio::test]
async fn resume_continues_from_a_tool_result_left_by_the_previous_turn_limit() {
    let call = ToolCall {
        id: "call-1".to_owned(),
        name: "inspect".to_owned(),
        arguments: json!({}),
        thought_signature: None,
        namespace: None,
    };
    let model = Arc::new(ScriptedModel::new(vec![
        Step::CallTool(call.clone()),
        Step::Respond("Finished."),
    ]));
    let tool = Arc::new(InspectTool::new());
    let mut agent = Agent::new(model.clone(), "Be concise.")
        .with_tools(vec![tool])
        .expect("unique tool name must be accepted");
    agent
        .set_config(AgentConfig {
            max_model_turns: 1,
            ..AgentConfig::default()
        })
        .expect("an empty queue must accept its configured limit");

    let error = agent
        .prompt("Inspect")
        .await
        .expect_err("first run must stop at its model-turn limit");
    assert!(matches!(error, AgentError::TurnLimit(1)));

    let result = agent
        .resume()
        .await
        .expect("tool-result tail must be resumable");

    assert_eq!(result.output, "Finished.");
    assert_eq!(
        model.requests()[1].messages.last(),
        Some(&Message::Tool {
            result: ToolResult {
                call_id: call.id,
                name: call.name,
                content: vec![ContentBlock::text("inspection result")],
                details: None,
                is_error: false,
            },
        })
    );
}

#[tokio::test]
async fn resume_rejects_an_empty_conversation() {
    let model = Arc::new(ScriptedModel::new(Vec::new()));
    let mut agent = Agent::new(model, "Be concise.");

    let error = agent
        .resume()
        .await
        .expect_err("empty conversation must not be sampled");

    assert!(matches!(error, AgentError::NothingToResume));
}

#[tokio::test]
async fn resume_rejects_an_assistant_tail_without_queued_input() {
    let model = Arc::new(ScriptedModel::new(vec![Step::Respond("Done.")]));
    let mut agent = Agent::new(model.clone(), "Be concise.");
    agent
        .prompt("Finish")
        .await
        .expect("initial prompt must complete");

    let error = agent
        .resume()
        .await
        .expect_err("assistant tail needs new user input");

    assert!(matches!(error, AgentError::AssistantTail));
    assert_eq!(model.requests().len(), 1);
}

#[tokio::test]
async fn resume_claims_queued_input_before_awaiting_lifecycle_listeners() {
    let model = Arc::new(ScriptedModel::new(vec![
        Step::Respond("Initial answer."),
        Step::Respond("Follow-up answer."),
    ]));
    let events = Arc::new(BlockingSecondAgentStart::new());
    let mut agent = Agent::new(model.clone(), "Be concise.").with_event_sink(events.clone());
    agent
        .prompt("Initial request")
        .await
        .expect("initial prompt must complete");
    let handle = agent.handle();
    handle
        .follow_up("Claimed follow-up")
        .expect("follow-up must fit");
    let mut resumed = Box::pin(agent.resume());

    tokio::select! {
        biased;
        result = &mut resumed => panic!("resume settled before listener release: {result:?}"),
        () = events.wait_until_blocked() => {}
    }
    handle.clear_follow_ups();
    events.release();
    resumed.await.expect("claimed follow-up must still run");

    assert_eq!(
        model.requests()[1].messages.last(),
        Some(&Message::user_text("Claimed follow-up"))
    );
}

#[tokio::test]
async fn queued_input_survives_a_turn_limit_for_resume() {
    let model = Arc::new(ScriptedModel::new(vec![
        Step::Respond("Initial answer."),
        Step::Respond("Follow-up answer."),
    ]));
    let mut agent = Agent::new(model.clone(), "Be concise.");
    agent
        .set_config(AgentConfig {
            max_model_turns: 1,
            ..AgentConfig::default()
        })
        .expect("an empty queue must accept its configured limit");
    let handle = agent.handle();
    let events = Arc::new(QueueFollowUpOnce::new(handle.clone()));
    let mut agent = agent.with_event_sink(events);

    let error = agent
        .prompt("Initial request")
        .await
        .expect_err("queued work must not bypass the turn limit");
    assert!(matches!(error, AgentError::TurnLimit(1)));
    assert!(handle.has_queued_messages());

    let result = agent
        .resume()
        .await
        .expect("resume must claim the retained follow-up");

    assert_eq!(result.output, "Follow-up answer.");
    assert!(!handle.has_queued_messages());
    assert_eq!(
        model.requests()[1].messages.last(),
        Some(&Message::user_text("Retained follow-up"))
    );
}

enum Step {
    Fail(&'static str),
    Respond(&'static str),
    CallTool(ToolCall),
}

struct ScriptedModel {
    steps: Mutex<VecDeque<Step>>,
    requests: Mutex<Vec<ModelRequest>>,
}

impl ScriptedModel {
    fn new(steps: Vec<Step>) -> Self {
        Self {
            steps: Mutex::new(steps.into()),
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
        match self
            .steps
            .lock()
            .expect("model step lock must not be poisoned")
            .pop_front()
            .expect("scripted step must exist")
        {
            Step::Fail(message) => {
                stream::once(async move { Err(ModelError::new(message)) }).boxed()
            }
            Step::Respond(text) => stream::once(async move {
                Ok(ModelEvent::Completed {
                    response: ModelResponse {
                        content: vec![AssistantContent::text(text)],
                        stop_reason: StopReason::Stop,
                        usage: None,
                        metadata: renoa_agent::AssistantMetadata::default(),
                    },
                })
            })
            .boxed(),
            Step::CallTool(call) => stream::once(async move {
                Ok(ModelEvent::Completed {
                    response: ModelResponse {
                        content: vec![AssistantContent::tool_call(call)],
                        stop_reason: StopReason::ToolUse,
                        usage: None,
                        metadata: renoa_agent::AssistantMetadata::default(),
                    },
                })
            })
            .boxed(),
        }
    }
}

struct InspectTool {
    spec: ToolSpec,
}

impl InspectTool {
    fn new() -> Self {
        Self {
            spec: ToolSpec {
                name: "inspect".to_owned(),
                description: "Inspect something.".to_owned(),
                input_schema: json!({ "type": "object" }),
            },
        }
    }
}

impl Tool for InspectTool {
    fn spec(&self) -> &ToolSpec {
        &self.spec
    }

    fn execute(
        &self,
        _call: ToolCall,
        _cancellation: CancellationToken,
        _updates: ToolUpdates,
    ) -> BoxFuture<'_, Result<ToolOutput, ToolError>> {
        Box::pin(async {
            Ok(ToolOutput {
                content: vec![ContentBlock::text("inspection result")],
                details: None,
            })
        })
    }
}

struct BlockingSecondAgentStart {
    starts: AtomicUsize,
    blocked: Semaphore,
    release: Semaphore,
}

impl BlockingSecondAgentStart {
    fn new() -> Self {
        Self {
            starts: AtomicUsize::new(0),
            blocked: Semaphore::new(0),
            release: Semaphore::new(0),
        }
    }

    async fn wait_until_blocked(&self) {
        self.blocked
            .acquire()
            .await
            .expect("blocked semaphore must remain open")
            .forget();
    }

    fn release(&self) {
        self.release.add_permits(1);
    }
}

impl AgentEventSink for BlockingSecondAgentStart {
    fn emit(&self, event: AgentEvent) -> BoxFuture<'_, ()> {
        Box::pin(async move {
            if event == AgentEvent::AgentStart && self.starts.fetch_add(1, Ordering::SeqCst) == 1 {
                self.blocked.add_permits(1);
                self.release
                    .acquire()
                    .await
                    .expect("release semaphore must remain open")
                    .forget();
            }
        })
    }
}

struct QueueFollowUpOnce {
    handle: renoa_agent::AgentHandle,
    queued: AtomicUsize,
}

impl QueueFollowUpOnce {
    fn new(handle: renoa_agent::AgentHandle) -> Self {
        Self {
            handle,
            queued: AtomicUsize::new(0),
        }
    }
}

impl AgentEventSink for QueueFollowUpOnce {
    fn emit(&self, event: AgentEvent) -> BoxFuture<'_, ()> {
        Box::pin(async move {
            if matches!(
                event,
                AgentEvent::MessageEnd {
                    message: Message::Assistant { .. }
                }
            ) && self.queued.fetch_add(1, Ordering::SeqCst) == 0
            {
                self.handle
                    .follow_up("Retained follow-up")
                    .expect("follow-up must fit");
            }
        })
    }
}
