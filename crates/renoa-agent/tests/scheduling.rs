use std::{
    collections::VecDeque,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
};

use futures_util::{StreamExt, stream};
use renoa_agent::{
    Agent, AgentConfig, AgentConfigError, AgentEvent, AgentEventSink, AgentHandle,
    AssistantContent, BoxFuture, ContentBlock, Message, Model, ModelEvent, ModelEventStream,
    ModelRequest, ModelResponse, QueueError, QueueMode, StopReason, Tool, ToolCall, ToolError,
    ToolOutput, ToolResult, ToolSpec, ToolUpdates,
};
use serde_json::json;
use tokio_util::sync::CancellationToken;

#[tokio::test]
async fn steering_waits_for_tool_results_then_enters_the_next_model_turn() {
    let call = ToolCall {
        id: "call-1".to_owned(),
        name: "inspect".to_owned(),
        arguments: json!({}),
        thought_signature: None,
        namespace: None,
    };
    let model = Arc::new(ScriptedModel::new(vec![
        ModelResponse {
            content: vec![
                AssistantContent::text("Inspecting."),
                AssistantContent::tool_call(call.clone()),
            ],
            stop_reason: StopReason::ToolUse,
            usage: None,
            metadata: renoa_agent::AssistantMetadata::default(),
        },
        text_response("Adjusted."),
    ]));
    let tool = Arc::new(InspectTool::new());
    let agent = Agent::new(model.clone(), "Be useful.")
        .with_tools(vec![tool])
        .expect("unique tool name must be accepted");
    let handle = agent.handle();
    let sink = Arc::new(SteerAfterFirstAssistant::new(handle));
    let mut agent = agent.with_event_sink(sink);

    let result = agent
        .prompt("Start")
        .await
        .expect("steered prompt must complete");

    assert_eq!(result.output, "Adjusted.");
    assert_eq!(result.model_turns, 2);
    assert_eq!(
        model.requests()[1].messages,
        vec![
            Message::user_text("Start"),
            Message::Assistant {
                content: vec![
                    AssistantContent::text("Inspecting."),
                    AssistantContent::tool_call(call.clone()),
                ],
                stop_reason: StopReason::ToolUse,
                usage: None,
                metadata: renoa_agent::AssistantMetadata::default(),
            },
            Message::Tool {
                result: ToolResult {
                    call_id: call.id,
                    name: call.name,
                    content: vec![ContentBlock::text("inspection result")],
                    details: None,
                    is_error: false,
                },
            },
            Message::user_text("Change direction"),
        ]
    );
}

#[tokio::test]
async fn follow_ups_run_one_at_a_time_after_the_agent_would_stop() {
    let model = Arc::new(ScriptedModel::new(vec![
        text_response("First answer."),
        text_response("Second answer."),
        text_response("Third answer."),
    ]));
    let agent = Agent::new(model.clone(), "Be useful.");
    let handle = agent.handle();
    let sink = Arc::new(FollowUpAfterFirstAssistant::new(handle));
    let mut agent = agent.with_event_sink(sink);

    let result = agent
        .prompt("Initial request")
        .await
        .expect("follow-up run must complete");

    assert_eq!(result.output, "Third answer.");
    assert_eq!(result.model_turns, 3);
    let requests = model.requests();
    assert_eq!(
        requests[1].messages.last(),
        Some(&Message::user_text("Follow-up one"))
    );
    assert_eq!(
        requests[2].messages.last(),
        Some(&Message::user_text("Follow-up two"))
    );
}

#[tokio::test]
async fn all_mode_batches_queued_follow_ups_into_one_model_turn() {
    let model = Arc::new(ScriptedModel::new(vec![
        text_response("First answer."),
        text_response("Combined answer."),
    ]));
    let mut agent = Agent::new(model.clone(), "Be useful.");
    agent
        .set_config(AgentConfig {
            follow_up_mode: QueueMode::All,
            ..AgentConfig::default()
        })
        .expect("an empty queue must accept its configured limit");
    let handle = agent.handle();
    let sink = Arc::new(FollowUpAfterFirstAssistant::new(handle));
    let mut agent = agent.with_event_sink(sink);

    let result = agent
        .prompt("Initial request")
        .await
        .expect("batched follow-up run must complete");

    assert_eq!(result.output, "Combined answer.");
    assert_eq!(result.model_turns, 2);
    assert_eq!(
        &model.requests()[1].messages[2..],
        &[
            Message::user_text("Follow-up one"),
            Message::user_text("Follow-up two"),
        ]
    );
}

#[tokio::test]
async fn steering_queue_drains_before_follow_ups() {
    let model = Arc::new(ScriptedModel::new(vec![
        text_response("Initial answer."),
        text_response("Steered once."),
        text_response("Steered twice."),
        text_response("Followed up."),
    ]));
    let agent = Agent::new(model.clone(), "Be useful.");
    let handle = agent.handle();
    let sink = Arc::new(MixedScheduleAfterFirstAssistant::new(handle));
    let mut agent = agent.with_event_sink(sink);

    let result = agent
        .prompt("Initial request")
        .await
        .expect("scheduled run must complete");

    assert_eq!(result.output, "Followed up.");
    let requests = model.requests();
    let scheduled = requests[1..]
        .iter()
        .map(|request| request.messages.last().expect("request must have a tail"))
        .collect::<Vec<_>>();
    assert_eq!(
        scheduled,
        vec![
            &Message::user_text("Steering one"),
            &Message::user_text("Steering two"),
            &Message::user_text("Follow-up"),
        ]
    );
}

#[test]
fn scheduling_queue_rejects_messages_beyond_its_configured_bound() {
    let model = Arc::new(ScriptedModel::new(Vec::new()));
    let mut agent = Agent::new(model, "Be useful.");
    agent
        .set_config(AgentConfig {
            max_queued_messages: 1,
            ..AgentConfig::default()
        })
        .expect("an empty queue must accept its configured limit");
    let handle = agent.handle();

    handle.steer("accepted").expect("first message must fit");
    assert_eq!(
        handle.follow_up("rejected"),
        Err(QueueError::Full { limit: 1 })
    );
}

#[test]
fn lowering_the_queue_limit_cannot_invalidate_accepted_input() {
    let model = Arc::new(ScriptedModel::new(Vec::new()));
    let mut agent = Agent::new(model, "Be useful.");
    let handle = agent.handle();
    handle
        .steer("accepted")
        .expect("default queue must accept input");

    let error = agent
        .set_config(AgentConfig {
            max_queued_messages: 0,
            ..AgentConfig::default()
        })
        .expect_err("a limit below pending input must be rejected");

    assert_eq!(
        error,
        AgentConfigError::QueueLimitBelowPending {
            pending: 1,
            limit: 0,
        }
    );
    assert!(handle.has_queued_messages());
}

#[test]
fn queued_input_can_be_inspected_and_cleared_by_kind() {
    let model = Arc::new(ScriptedModel::new(Vec::new()));
    let agent = Agent::new(model, "Be useful.");
    let handle = agent.handle();

    handle.steer("steering").expect("steering must fit");
    handle.follow_up("later").expect("follow-up must fit");
    assert!(handle.has_queued_messages());

    handle.clear_steering();
    assert!(handle.has_queued_messages(), "follow-up must remain queued");

    handle.clear_follow_ups();
    assert!(!handle.has_queued_messages());

    handle.steer("steering").expect("steering must fit");
    handle.follow_up("later").expect("follow-up must fit");
    handle.clear_all_queued_messages();
    assert!(!handle.has_queued_messages());
}

#[test]
fn scheduling_rejects_input_after_its_agent_is_dropped() {
    let model = Arc::new(ScriptedModel::new(Vec::new()));
    let handle = {
        let agent = Agent::new(model, "Be useful.");
        agent.handle()
    };

    assert_eq!(handle.steer("orphaned"), Err(QueueError::Closed));
    assert_eq!(handle.follow_up("orphaned"), Err(QueueError::Closed));
}

struct ScriptedModel {
    responses: Mutex<VecDeque<ModelResponse>>,
    requests: Mutex<Vec<ModelRequest>>,
}

impl ScriptedModel {
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
            .responses
            .lock()
            .expect("model response lock must not be poisoned")
            .pop_front()
            .expect("scripted response must exist");
        stream::once(async { Ok(ModelEvent::Completed { response }) }).boxed()
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

struct SteerAfterFirstAssistant {
    handle: AgentHandle,
    queued: AtomicBool,
}

impl SteerAfterFirstAssistant {
    fn new(handle: AgentHandle) -> Self {
        Self {
            handle,
            queued: AtomicBool::new(false),
        }
    }
}

impl AgentEventSink for SteerAfterFirstAssistant {
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
                    .steer("Change direction")
                    .expect("steering queue must accept the message");
            }
        })
    }
}

struct FollowUpAfterFirstAssistant {
    handle: AgentHandle,
    queued: AtomicBool,
}

impl FollowUpAfterFirstAssistant {
    fn new(handle: AgentHandle) -> Self {
        Self {
            handle,
            queued: AtomicBool::new(false),
        }
    }
}

impl AgentEventSink for FollowUpAfterFirstAssistant {
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
                    .follow_up("Follow-up one")
                    .expect("first follow-up must be queued");
                self.handle
                    .follow_up("Follow-up two")
                    .expect("second follow-up must be queued");
            }
        })
    }
}

struct MixedScheduleAfterFirstAssistant {
    handle: AgentHandle,
    queued: AtomicBool,
}

impl MixedScheduleAfterFirstAssistant {
    fn new(handle: AgentHandle) -> Self {
        Self {
            handle,
            queued: AtomicBool::new(false),
        }
    }
}

impl AgentEventSink for MixedScheduleAfterFirstAssistant {
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
                    .follow_up("Follow-up")
                    .expect("follow-up must be queued");
                self.handle
                    .steer("Steering one")
                    .expect("first steering message must be queued");
                self.handle
                    .steer("Steering two")
                    .expect("second steering message must be queued");
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
