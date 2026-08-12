use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, AtomicUsize, Ordering},
};

use futures_util::{StreamExt, stream};
use renoa_agent::{
    Agent, AgentConfig, AgentError, AgentEvent, AgentEventSink, AgentHandle, AssistantContent,
    BoxFuture, ContentBlock, Message, Model, ModelEvent, ModelEventStream, ModelRequest,
    ModelResponse, StopReason, Tool, ToolCall, ToolError, ToolExecutionMode, ToolOutput, ToolSpec,
    ToolUpdates,
};
use serde_json::json;
use tokio::sync::Semaphore;
use tokio_util::sync::CancellationToken;

#[tokio::test]
async fn parallel_tools_finish_as_ready_but_enter_history_in_source_order() {
    let first = tool_call("first", "work");
    let second = tool_call("second", "work");
    let model = Arc::new(ScriptedModel::new(vec![
        tool_response(vec![first.clone(), second.clone()]),
        text_response("done"),
    ]));
    let release_first = Arc::new(Semaphore::new(0));
    let tool = Arc::new(OrderedCompletionTool::new(release_first.clone()));
    let events = Arc::new(CompletionSink::new(release_first));
    let mut agent = Agent::new(model.clone(), "Work carefully.")
        .with_tools(vec![tool])
        .expect("tool name is unique")
        .with_event_sink(events.clone());
    agent
        .set_config(AgentConfig {
            tool_execution: ToolExecutionMode::Parallel,
            ..AgentConfig::default()
        })
        .expect("config is valid");

    tokio::time::timeout(std::time::Duration::from_secs(1), agent.prompt("work"))
        .await
        .expect("parallel execution must not deadlock")
        .expect("run must complete");

    assert_eq!(events.completions(), vec!["second", "first"]);
    assert_eq!(
        &model.requests()[1].messages[2..],
        &[
            Message::Tool {
                result: tool_result(&first),
            },
            Message::Tool {
                result: tool_result(&second),
            },
        ]
    );
}

#[tokio::test]
async fn one_sequential_tool_serializes_the_entire_batch() {
    let activity = Arc::new(Activity::default());
    let model = Arc::new(ScriptedModel::new(vec![
        tool_response(vec![
            tool_call("first", "exclusive"),
            tool_call("second", "shared"),
        ]),
        text_response("done"),
    ]));
    let mut agent = Agent::new(model, "Work carefully.")
        .with_tools(vec![
            Arc::new(YieldingTool::new(
                "exclusive",
                ToolExecutionMode::Sequential,
                activity.clone(),
            )),
            Arc::new(YieldingTool::new(
                "shared",
                ToolExecutionMode::Parallel,
                activity.clone(),
            )),
        ])
        .expect("tool names are unique");
    agent
        .set_config(AgentConfig {
            tool_execution: ToolExecutionMode::Parallel,
            ..AgentConfig::default()
        })
        .expect("config is valid");

    agent.prompt("work").await.expect("run must complete");

    assert!(!activity.overlapped.load(Ordering::SeqCst));
}

#[tokio::test]
async fn parallel_cancellation_settles_every_call_in_source_order() {
    let first = tool_call("first", "blocking");
    let second = tool_call("second", "blocking");
    let model = Arc::new(ScriptedModel::new(vec![tool_response(vec![
        first.clone(),
        second.clone(),
    ])]));
    let agent = Agent::new(model, "Work carefully.")
        .with_tools(vec![Arc::new(CancellableTool::new())])
        .expect("tool name is unique");
    let handle = agent.handle();
    let mut agent = agent.with_event_sink(Arc::new(AbortOnUpdate { handle }));
    agent
        .set_config(AgentConfig {
            tool_execution: ToolExecutionMode::Parallel,
            ..AgentConfig::default()
        })
        .expect("config is valid");

    let error = agent
        .prompt("work")
        .await
        .expect_err("progress listener must cancel the run");

    assert!(matches!(error, AgentError::Cancelled));
    let results = &agent.state().messages()[2..];
    assert_eq!(results.len(), 2);
    for (message, call) in results.iter().zip([first, second]) {
        let Message::Tool { result } = message else {
            panic!("tool result expected");
        };
        assert_eq!(result.call_id, call.id);
        assert!(result.is_error);
    }
}

#[test]
fn tool_execution_is_sequential_by_default() {
    assert_eq!(
        AgentConfig::default().tool_execution,
        ToolExecutionMode::Sequential
    );
}

struct OrderedCompletionTool {
    spec: ToolSpec,
    release_first: Arc<Semaphore>,
}

impl OrderedCompletionTool {
    fn new(release_first: Arc<Semaphore>) -> Self {
        Self {
            spec: tool_spec("work"),
            release_first,
        }
    }
}

impl Tool for OrderedCompletionTool {
    fn spec(&self) -> &ToolSpec {
        &self.spec
    }

    fn execute(
        &self,
        call: ToolCall,
        _cancellation: CancellationToken,
        _updates: ToolUpdates,
    ) -> BoxFuture<'_, Result<ToolOutput, ToolError>> {
        Box::pin(async move {
            if call.id == "first" {
                self.release_first
                    .acquire()
                    .await
                    .expect("release semaphore must remain open")
                    .forget();
            }
            Ok(tool_output(&call.id))
        })
    }
}

struct CancellableTool {
    spec: ToolSpec,
}

impl CancellableTool {
    fn new() -> Self {
        Self {
            spec: tool_spec("blocking"),
        }
    }
}

impl Tool for CancellableTool {
    fn spec(&self) -> &ToolSpec {
        &self.spec
    }

    fn execute(
        &self,
        _call: ToolCall,
        cancellation: CancellationToken,
        updates: ToolUpdates,
    ) -> BoxFuture<'_, Result<ToolOutput, ToolError>> {
        Box::pin(async move {
            updates.emit(tool_output("started")).await;
            cancellation.cancelled().await;
            Ok(tool_output("unreachable"))
        })
    }
}

struct AbortOnUpdate {
    handle: AgentHandle,
}

impl AgentEventSink for AbortOnUpdate {
    fn emit(&self, event: AgentEvent) -> BoxFuture<'_, ()> {
        Box::pin(async move {
            if matches!(event, AgentEvent::ToolExecutionUpdate { .. }) {
                self.handle.abort();
            }
        })
    }
}

#[derive(Default)]
struct Activity {
    active: AtomicUsize,
    overlapped: AtomicBool,
}

struct YieldingTool {
    spec: ToolSpec,
    mode: ToolExecutionMode,
    activity: Arc<Activity>,
}

impl YieldingTool {
    fn new(name: &str, mode: ToolExecutionMode, activity: Arc<Activity>) -> Self {
        Self {
            spec: tool_spec(name),
            mode,
            activity,
        }
    }
}

impl Tool for YieldingTool {
    fn spec(&self) -> &ToolSpec {
        &self.spec
    }

    fn execution_mode(&self) -> ToolExecutionMode {
        self.mode
    }

    fn execute(
        &self,
        call: ToolCall,
        _cancellation: CancellationToken,
        _updates: ToolUpdates,
    ) -> BoxFuture<'_, Result<ToolOutput, ToolError>> {
        Box::pin(async move {
            self.activity.active.fetch_add(1, Ordering::SeqCst);
            tokio::task::yield_now().await;
            if self.activity.active.load(Ordering::SeqCst) > 1 {
                self.activity.overlapped.store(true, Ordering::SeqCst);
            }
            self.activity.active.fetch_sub(1, Ordering::SeqCst);
            Ok(tool_output(&call.id))
        })
    }
}

struct CompletionSink {
    completions: Mutex<Vec<String>>,
    release_first: Arc<Semaphore>,
}

impl CompletionSink {
    fn new(release_first: Arc<Semaphore>) -> Self {
        Self {
            completions: Mutex::new(Vec::new()),
            release_first,
        }
    }

    fn completions(&self) -> Vec<String> {
        self.completions.lock().expect("event lock").clone()
    }
}

impl AgentEventSink for CompletionSink {
    fn emit(&self, event: AgentEvent) -> BoxFuture<'_, ()> {
        Box::pin(async move {
            if let AgentEvent::ToolExecutionEnd { call, .. } = event {
                self.completions
                    .lock()
                    .expect("event lock")
                    .push(call.id.clone());
                if call.id == "second" {
                    self.release_first.add_permits(1);
                }
            }
        })
    }
}

struct ScriptedModel {
    responses: Mutex<std::collections::VecDeque<ModelResponse>>,
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
        self.requests.lock().expect("request lock").clone()
    }
}

impl Model for ScriptedModel {
    fn stream(
        &self,
        request: ModelRequest,
        _cancellation: CancellationToken,
    ) -> ModelEventStream<'_> {
        self.requests.lock().expect("request lock").push(request);
        let response = self
            .responses
            .lock()
            .expect("response lock")
            .pop_front()
            .expect("scripted response");
        stream::once(async { Ok(ModelEvent::Completed { response }) }).boxed()
    }
}

fn tool_call(id: &str, name: &str) -> ToolCall {
    ToolCall {
        id: id.to_owned(),
        name: name.to_owned(),
        arguments: json!({}),
        thought_signature: None,
        namespace: None,
    }
}

fn tool_response(calls: Vec<ToolCall>) -> ModelResponse {
    ModelResponse {
        content: calls.into_iter().map(AssistantContent::tool_call).collect(),
        stop_reason: StopReason::ToolUse,
        usage: None,
        metadata: renoa_agent::AssistantMetadata::default(),
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

fn tool_spec(name: &str) -> ToolSpec {
    ToolSpec {
        name: name.to_owned(),
        description: "Fixture tool.".to_owned(),
        input_schema: json!({ "type": "object" }),
    }
}

fn tool_output(id: &str) -> ToolOutput {
    ToolOutput {
        content: vec![ContentBlock::text(id)],
        details: None,
    }
}

fn tool_result(call: &ToolCall) -> renoa_agent::ToolResult {
    renoa_agent::ToolResult {
        call_id: call.id.clone(),
        name: call.name.clone(),
        content: vec![ContentBlock::text(&call.id)],
        details: None,
        is_error: false,
    }
}
