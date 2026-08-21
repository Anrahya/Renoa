use std::sync::{Arc, Mutex};

use futures_util::{StreamExt, stream};
use renoa_agent::{
    Agent, AgentConfig, AgentConfigError, AgentError, AgentEvent, AgentEventSink, AgentState,
    AssistantContent, BoxFuture, ContentBlock, Message, MessageRole, Model, ModelEvent,
    ModelEventStream, ModelRequest, ModelResponse, StopReason, TokenUsage, Tool, ToolCall,
    ToolError, ToolOutput, ToolResult, ToolSpec, ToolUpdates,
};
use serde_json::json;
use tokio_util::sync::CancellationToken;

const TOOL_TURN_USAGE: TokenUsage = TokenUsage {
    input: 10,
    output: 3,
    cache_read: 4,
    cache_write: 1,
};
const FINAL_TURN_USAGE: TokenUsage = TokenUsage {
    input: 6,
    output: 2,
    cache_read: 8,
    cache_write: 0,
};
const RUN_USAGE: TokenUsage = TokenUsage {
    input: 16,
    output: 5,
    cache_read: 12,
    cache_write: 1,
};

#[tokio::test]
async fn ordered_content_survives_tool_execution_and_continuation() {
    let call = ToolCall {
        id: "call-1".to_owned(),
        name: "read_file".to_owned(),
        arguments: json!({ "path": "notes.txt" }),
        thought_signature: None,
        namespace: None,
    };
    let model = Arc::new(ScriptedModel::new(vec![
        ModelResponse {
            content: vec![
                AssistantContent::text("I will inspect first. "),
                AssistantContent::tool_call(call.clone()),
                AssistantContent::text("Then I will answer."),
            ],
            stop_reason: StopReason::ToolUse,
            usage: Some(TOOL_TURN_USAGE),
            metadata: renoa_agent::AssistantMetadata::default(),
        },
        ModelResponse {
            content: vec![AssistantContent::text("The file contains alpha.")],
            stop_reason: StopReason::Stop,
            usage: Some(FINAL_TURN_USAGE),
            metadata: renoa_agent::AssistantMetadata::default(),
        },
    ]));
    let tool = Arc::new(ReadFileTool::new());
    let events = Arc::new(RecordingSink::default());
    let mut agent = Agent::new(model.clone(), "Use tools when needed.")
        .with_tools(vec![tool.clone()])
        .expect("unique tool names must be accepted")
        .with_event_sink(events.clone());

    let result = agent
        .prompt("Read notes.txt")
        .await
        .expect("tool prompt must complete");

    assert_eq!(result.output, "The file contains alpha.");
    assert_eq!(result.model_turns, 2);
    assert_eq!(result.stop_reason, StopReason::Stop);
    assert_eq!(result.usage, Some(RUN_USAGE));
    assert_eq!(tool.calls(), vec![call.clone()]);
    let tool_result = ToolResult {
        call_id: call.id.clone(),
        name: call.name.clone(),
        content: vec![ContentBlock::text("alpha")],
        details: None,
        is_error: false,
    };
    let expected_events = [
        AgentEvent::ToolExecutionStart { call: call.clone() },
        AgentEvent::ToolExecutionEnd {
            call: call.clone(),
            result: tool_result.clone(),
        },
        AgentEvent::MessageStart {
            role: MessageRole::Tool,
        },
    ];
    assert!(
        events
            .events()
            .windows(expected_events.len())
            .any(|window| window == expected_events)
    );
    let requests = model.requests();
    let user = Message::user_text("Read notes.txt");
    assert_eq!(requests[0].messages, vec![user.clone()]);
    assert_eq!(
        requests[1].messages,
        vec![
            user,
            Message::Assistant {
                content: vec![
                    AssistantContent::text("I will inspect first. "),
                    AssistantContent::tool_call(call),
                    AssistantContent::text("Then I will answer."),
                ],
                stop_reason: StopReason::ToolUse,
                usage: Some(TOOL_TURN_USAGE),
                metadata: renoa_agent::AssistantMetadata::default(),
            },
            Message::Tool {
                result: tool_result,
            },
        ]
    );
    assert!(requests.iter().all(|request| {
        request.system_prompt == "Use tools when needed." && request.tools == vec![read_file_spec()]
    }));
    let encoded = serde_json::to_string(agent.state()).expect("agent state must serialize");
    let restored: AgentState = serde_json::from_str(&encoded).expect("agent state must restore");
    assert_eq!(&restored, agent.state());
}

#[tokio::test]
async fn unavailable_tool_becomes_a_model_visible_error() {
    let call = tool_call("missing_tool");
    let model = Arc::new(ScriptedModel::new(vec![
        tool_response(call.clone(), StopReason::ToolUse),
        text_response("Recovered."),
    ]));
    let mut agent = Agent::new(model.clone(), "Be useful.");

    let result = agent
        .prompt("Use the missing tool")
        .await
        .expect("the model must be allowed to recover");

    assert_eq!(result.output, "Recovered.");
    assert_eq!(
        model.requests()[1].messages.last(),
        Some(&Message::Tool {
            result: ToolResult {
                call_id: call.id,
                name: call.name,
                content: vec![ContentBlock::text("Tool `missing_tool` is not available.")],
                details: Some(json!({
                    "error": {
                        "code": "unavailable",
                        "partial_changes_possible": false
                    }
                })),
                is_error: true,
            },
        })
    );
}

#[tokio::test]
async fn tool_failure_becomes_a_model_visible_error() {
    let call = tool_call("failing_tool");
    let model = Arc::new(ScriptedModel::new(vec![
        tool_response(call.clone(), StopReason::ToolUse),
        text_response("Recovered."),
    ]));
    let tool = Arc::new(FailingTool::new());
    let mut agent = Agent::new(model.clone(), "Be useful.")
        .with_tools(vec![tool])
        .expect("unique tool names must be accepted");

    agent
        .prompt("Use the failing tool")
        .await
        .expect("the model must be allowed to recover");

    assert_eq!(
        model.requests()[1].messages.last(),
        Some(&Message::Tool {
            result: ToolResult {
                call_id: call.id,
                name: call.name,
                content: vec![ContentBlock::text("fixture failed")],
                details: Some(json!({
                    "error": {
                        "code": "internal",
                        "partial_changes_possible": false
                    }
                })),
                is_error: true,
            },
        })
    );
}

#[tokio::test]
async fn length_stopped_tool_arguments_are_never_executed() {
    let call = tool_call("read_file");
    let model = Arc::new(ScriptedModel::new(vec![
        tool_response(call.clone(), StopReason::Length),
        text_response("Retried safely."),
    ]));
    let tool = Arc::new(ReadFileTool::new());
    let mut agent = Agent::new(model.clone(), "Be careful.")
        .with_tools(vec![tool.clone()])
        .expect("unique tool names must be accepted");

    agent
        .prompt("Read a file")
        .await
        .expect("the model must receive the truncation error");

    assert!(tool.calls().is_empty());
    assert_eq!(
        model.requests()[1].messages.last(),
        Some(&Message::Tool {
            result: ToolResult {
                call_id: call.id,
                name: call.name,
                content: vec![ContentBlock::text(
                    "Tool call was not executed because the model response reached its token limit.",
                )],
                details: Some(json!({
                    "error": {
                        "code": "output_limit",
                        "partial_changes_possible": false
                    }
                })),
                is_error: true,
            },
        })
    );
}

#[test]
fn duplicate_tool_names_are_rejected() {
    let model = Arc::new(ScriptedModel::new(Vec::new()));
    let result = Agent::new(model, "Be useful.").with_tools(vec![
        Arc::new(ReadFileTool::new()),
        Arc::new(ReadFileTool::new()),
    ]);

    assert!(matches!(
        result,
        Err(AgentConfigError::DuplicateToolName(name)) if name == "read_file"
    ));
}

#[tokio::test]
async fn configured_turn_limit_stops_a_tool_loop() {
    let call = tool_call("read_file");
    let model = Arc::new(ScriptedModel::new(vec![tool_response(
        call.clone(),
        StopReason::ToolUse,
    )]));
    let tool = Arc::new(ReadFileTool::new());
    let mut agent = Agent::new(model.clone(), "Be bounded.")
        .with_tools(vec![tool.clone()])
        .expect("unique tool names must be accepted");
    agent
        .set_config(AgentConfig {
            max_model_turns: 1,
            ..AgentConfig::default()
        })
        .expect("an empty queue must accept its configured limit");

    let error = agent
        .prompt("Loop")
        .await
        .expect_err("the turn limit must stop continuation");

    assert!(matches!(error, AgentError::TurnLimit(1)));
    assert_eq!(model.requests().len(), 1);
    assert_eq!(tool.calls(), vec![call]);
}

#[tokio::test]
async fn configured_tool_call_limit_prevents_execution() {
    let call = tool_call("read_file");
    let mut response = tool_response(call, StopReason::ToolUse);
    response.usage = Some(TOOL_TURN_USAGE);
    let model = Arc::new(ScriptedModel::new(vec![response]));
    let tool = Arc::new(ReadFileTool::new());
    let mut agent = Agent::new(model, "Be bounded.")
        .with_tools(vec![tool.clone()])
        .expect("unique tool names must be accepted");
    agent
        .set_config(AgentConfig {
            max_tool_calls_per_turn: 0,
            ..AgentConfig::default()
        })
        .expect("an empty queue must accept its configured limit");

    let error = agent
        .prompt("Do not execute")
        .await
        .expect_err("the tool call limit must reject the response");

    assert!(matches!(
        error,
        AgentError::ToolCallLimit {
            actual: 1,
            limit: 0,
            usage: Some(TOOL_TURN_USAGE),
        }
    ));
    assert!(tool.calls().is_empty());
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

struct ReadFileTool {
    spec: ToolSpec,
    calls: Mutex<Vec<ToolCall>>,
}

impl ReadFileTool {
    fn new() -> Self {
        Self {
            spec: read_file_spec(),
            calls: Mutex::new(Vec::new()),
        }
    }

    fn calls(&self) -> Vec<ToolCall> {
        self.calls
            .lock()
            .expect("tool call lock must not be poisoned")
            .clone()
    }
}

impl Tool for ReadFileTool {
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
            self.calls
                .lock()
                .expect("tool call lock must not be poisoned")
                .push(call);
            Ok(ToolOutput {
                content: vec![ContentBlock::text("alpha")],
                details: None,
            })
        })
    }
}

struct FailingTool {
    spec: ToolSpec,
}

impl FailingTool {
    fn new() -> Self {
        Self {
            spec: ToolSpec {
                name: "failing_tool".to_owned(),
                description: "Always fails.".to_owned(),
                input_schema: json!({ "type": "object" }),
            },
        }
    }
}

impl Tool for FailingTool {
    fn spec(&self) -> &ToolSpec {
        &self.spec
    }

    fn execute(
        &self,
        _call: ToolCall,
        _cancellation: CancellationToken,
        _updates: ToolUpdates,
    ) -> BoxFuture<'_, Result<ToolOutput, ToolError>> {
        Box::pin(async { Err(ToolError::new("fixture failed")) })
    }
}

fn tool_call(name: &str) -> ToolCall {
    ToolCall {
        id: "call-1".to_owned(),
        name: name.to_owned(),
        arguments: json!({}),
        thought_signature: None,
        namespace: None,
    }
}

fn tool_response(call: ToolCall, stop_reason: StopReason) -> ModelResponse {
    ModelResponse {
        content: vec![AssistantContent::tool_call(call)],
        stop_reason,
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

fn read_file_spec() -> ToolSpec {
    ToolSpec {
        name: "read_file".to_owned(),
        description: "Read a UTF-8 file.".to_owned(),
        input_schema: json!({
            "type": "object",
            "properties": {
                "path": { "type": "string" }
            },
            "required": ["path"]
        }),
    }
}

#[derive(Default)]
struct RecordingSink {
    events: Mutex<Vec<AgentEvent>>,
}

impl RecordingSink {
    fn events(&self) -> Vec<AgentEvent> {
        self.events
            .lock()
            .expect("event sink lock must not be poisoned")
            .clone()
    }
}

impl AgentEventSink for RecordingSink {
    fn emit(&self, event: AgentEvent) -> BoxFuture<'_, ()> {
        Box::pin(async move {
            self.events
                .lock()
                .expect("event sink lock must not be poisoned")
                .push(event);
        })
    }
}
