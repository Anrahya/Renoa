use std::{
    collections::VecDeque,
    num::NonZeroU32,
    sync::{Arc, Mutex},
};

use futures_util::{StreamExt, stream};
use renoa_agent::{
    AssistantContent, AssistantMetadata, BoxFuture, ContentBlock, Message, Model, ModelError,
    ModelEvent, ModelEventStream, ModelRequest, ModelResponse, StopReason, Tool, ToolCall,
    ToolError, ToolOutput, ToolSpec, ToolUpdates,
};
use renoa_agent_loop::{
    AgentCommand, AgentLoopConfig, AgentToolBinding, MESSAGE_EVENT_KIND, ModelBinding,
    build_runtime,
};
use renoa_kernel::{
    AgentId, Command, CommandId, DriveResult, EffectRecovery, EventCursor, Kernel,
    OperationOutcome, SessionId,
};
use tempfile::tempdir;
use tokio_util::sync::CancellationToken;

#[tokio::test]
async fn kernel_drives_a_complete_model_tool_model_turn() {
    let directory = tempdir().expect("temporary directory");
    let kernel = Kernel::open(directory.path().join("kernel.sqlite3")).expect("open kernel");
    let agent_id = AgentId::new();
    let session_id = SessionId::new();
    kernel.create_agent(agent_id).expect("create agent");
    kernel
        .create_session(session_id, agent_id)
        .expect("create session");

    let calls = Arc::new(Mutex::new(Vec::new()));
    let model = Arc::new(ScriptedModel::new(
        [
            tool_response(tool_call(
                "replace-1",
                "replace_value",
                serde_json::json!({"from": "old", "to": "new"}),
            )),
            text_response("Changed old to new."),
        ],
        Arc::clone(&calls),
    ));
    let runtime = build_runtime(
        AgentLoopConfig::new(
            "Change the requested value.",
            NonZeroU32::new(4).expect("non-zero model limit"),
            NonZeroU32::new(4).expect("non-zero tool limit"),
        ),
        ModelBinding::new("scripted-model-v1", model, EffectRecovery::SafeToReplay),
        vec![AgentToolBinding::new(
            "replace-value-v1",
            Arc::new(ReplaceValue),
            EffectRecovery::NeverReplay,
        )],
    )
    .expect("build runtime");
    let content = serde_json::to_value(AgentCommand::text("Replace old with new."))
        .expect("serialize command");
    let admission = kernel
        .submit(session_id, Command::new(CommandId::new(), content))
        .expect("submit command");

    assert_eq!(
        kernel
            .drive(session_id, &runtime)
            .await
            .expect("drive operation"),
        DriveResult::Finished {
            operation_id: admission.operation_id,
            outcome: OperationOutcome::Completed,
        }
    );

    let requests = calls.lock().expect("request lock");
    assert_eq!(requests.len(), 2);
    assert_eq!(
        requests[0].messages,
        vec![Message::user_text("Replace old with new.")]
    );
    assert_eq!(requests[1].messages.len(), 3);
    assert!(matches!(requests[1].messages[1], Message::Assistant { .. }));
    assert!(matches!(requests[1].messages[2], Message::Tool { .. }));
    drop(requests);

    let page = kernel
        .events_after(session_id, EventCursor::START)
        .expect("read semantic history");
    assert_eq!(page.events.len(), 4);
    assert!(
        page.events
            .iter()
            .all(|event| event.kind == MESSAGE_EVENT_KIND)
    );
    let messages = page
        .events
        .iter()
        .map(|event| {
            serde_json::from_value::<Message>(event.payload.clone()).expect("decode message event")
        })
        .collect::<Vec<_>>();
    assert_eq!(messages[0], Message::user_text("Replace old with new."));
    assert!(matches!(messages[1], Message::Assistant { .. }));
    assert!(matches!(messages[2], Message::Tool { .. }));
    assert!(matches!(messages[3], Message::Assistant { .. }));

    let snapshot = kernel.inspect(session_id).expect("inspect session");
    let effects = &snapshot.operations[0].effects;
    assert_eq!(effects.len(), 3);
    assert_eq!(effects[0].recovery, EffectRecovery::SafeToReplay);
    assert_eq!(effects[1].recovery, EffectRecovery::NeverReplay);
    assert_eq!(effects[2].recovery, EffectRecovery::SafeToReplay);
}

#[tokio::test]
async fn duplicate_tool_call_identifiers_fail_before_any_tool_effect() {
    let directory = tempdir().expect("temporary directory");
    let kernel = Kernel::open(directory.path().join("kernel.sqlite3")).expect("open kernel");
    let agent_id = AgentId::new();
    let session_id = SessionId::new();
    kernel.create_agent(agent_id).expect("create agent");
    kernel
        .create_session(session_id, agent_id)
        .expect("create session");
    let requests = Arc::new(Mutex::new(Vec::new()));
    let duplicate_id = "replace-duplicate";
    let response = ModelResponse {
        content: vec![
            AssistantContent::tool_call(tool_call(
                duplicate_id,
                "replace_value",
                serde_json::json!({"from": "old", "to": "first"}),
            )),
            AssistantContent::tool_call(tool_call(
                duplicate_id,
                "replace_value",
                serde_json::json!({"from": "old", "to": "second"}),
            )),
        ],
        stop_reason: StopReason::ToolUse,
        usage: None,
        metadata: AssistantMetadata::default(),
    };
    let runtime = build_runtime(
        AgentLoopConfig::new(
            "Change the requested value.",
            NonZeroU32::new(2).expect("non-zero model limit"),
            NonZeroU32::new(2).expect("non-zero tool limit"),
        ),
        ModelBinding::new(
            "scripted-model-v1",
            Arc::new(ScriptedModel::new([response], requests)),
            EffectRecovery::SafeToReplay,
        ),
        vec![AgentToolBinding::new(
            "replace-value-v1",
            Arc::new(NeverCalledTool),
            EffectRecovery::NeverReplay,
        )],
    )
    .expect("build runtime");
    let command = serde_json::to_value(AgentCommand::text("Replace the value twice."))
        .expect("serialize command");
    let admission = kernel
        .submit(session_id, Command::new(CommandId::new(), command))
        .expect("submit command");

    assert!(matches!(
        kernel
            .drive(session_id, &runtime)
            .await
            .expect("reject duplicate tool calls"),
        DriveResult::Finished {
            operation_id,
            outcome: OperationOutcome::Failed { ref reason },
        } if operation_id == admission.operation_id
            && reason == "model returned duplicate tool-call identifier `replace-duplicate`"
    ));
    let snapshot = kernel
        .inspect(session_id)
        .expect("inspect failed operation");
    assert_eq!(snapshot.operations[0].effects.len(), 1);
    let history = kernel
        .events_after(session_id, EventCursor::START)
        .expect("read failed history");
    assert_eq!(history.events.len(), 1);
    assert!(matches!(
        serde_json::from_value::<Message>(history.events[0].payload.clone())
            .expect("decode user event"),
        Message::User { .. }
    ));
}

#[tokio::test]
async fn empty_tool_call_identifier_fails_before_any_tool_effect() {
    let directory = tempdir().expect("temporary directory");
    let kernel = Kernel::open(directory.path().join("kernel.sqlite3")).expect("open kernel");
    let agent_id = AgentId::new();
    let session_id = SessionId::new();
    kernel.create_agent(agent_id).expect("create agent");
    kernel
        .create_session(session_id, agent_id)
        .expect("create session");
    let response = tool_response(tool_call(
        "",
        "replace_value",
        serde_json::json!({"from": "old", "to": "new"}),
    ));
    let runtime = build_runtime(
        AgentLoopConfig::new(
            "Change the requested value.",
            NonZeroU32::new(2).expect("non-zero model limit"),
            NonZeroU32::new(1).expect("non-zero tool limit"),
        ),
        ModelBinding::new(
            "scripted-model-v1",
            Arc::new(ScriptedModel::new(
                [response],
                Arc::new(Mutex::new(Vec::new())),
            )),
            EffectRecovery::SafeToReplay,
        ),
        vec![AgentToolBinding::new(
            "replace-value-v1",
            Arc::new(NeverCalledTool),
            EffectRecovery::NeverReplay,
        )],
    )
    .expect("build runtime");
    let command =
        serde_json::to_value(AgentCommand::text("Replace the value.")).expect("serialize command");
    let admission = kernel
        .submit(session_id, Command::new(CommandId::new(), command))
        .expect("submit command");

    assert!(matches!(
        kernel
            .drive(session_id, &runtime)
            .await
            .expect("reject empty tool-call identifier"),
        DriveResult::Finished {
            operation_id,
            outcome: OperationOutcome::Failed { ref reason },
        } if operation_id == admission.operation_id
            && reason == "model returned a tool call with an empty identifier"
    ));
    let snapshot = kernel
        .inspect(session_id)
        .expect("inspect failed operation");
    assert_eq!(snapshot.operations[0].effects.len(), 1);
    let history = kernel
        .events_after(session_id, EventCursor::START)
        .expect("read failed history");
    assert_eq!(history.events.len(), 1);
    assert!(matches!(
        serde_json::from_value::<Message>(history.events[0].payload.clone())
            .expect("decode user event"),
        Message::User { .. }
    ));
}

struct ScriptedModel {
    responses: Mutex<VecDeque<ModelResponse>>,
    requests: Arc<Mutex<Vec<ModelRequest>>>,
}

impl ScriptedModel {
    fn new(
        responses: impl IntoIterator<Item = ModelResponse>,
        requests: Arc<Mutex<Vec<ModelRequest>>>,
    ) -> Self {
        Self {
            responses: Mutex::new(responses.into_iter().collect()),
            requests,
        }
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
            .ok_or_else(|| ModelError::new("scripted model ran out of responses"));
        stream::once(async move { response.map(|response| ModelEvent::Completed { response }) })
            .boxed()
    }
}

struct ReplaceValue;

impl Tool for ReplaceValue {
    fn spec(&self) -> &ToolSpec {
        static SPEC: std::sync::OnceLock<ToolSpec> = std::sync::OnceLock::new();
        SPEC.get_or_init(|| ToolSpec {
            name: "replace_value".to_owned(),
            description: "Replace one value.".to_owned(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "from": {"type": "string"},
                    "to": {"type": "string"}
                },
                "required": ["from", "to"],
                "additionalProperties": false
            }),
        })
    }

    fn execute(
        &self,
        call: ToolCall,
        _cancellation: CancellationToken,
        _updates: ToolUpdates,
    ) -> BoxFuture<'_, Result<ToolOutput, ToolError>> {
        Box::pin(std::future::ready(Ok(ToolOutput {
            content: vec![ContentBlock::text(format!(
                "Replaced {} with {}.",
                call.arguments["from"].as_str().unwrap_or_default(),
                call.arguments["to"].as_str().unwrap_or_default()
            ))],
            details: None,
        })))
    }
}

struct NeverCalledTool;

impl Tool for NeverCalledTool {
    fn spec(&self) -> &ToolSpec {
        static SPEC: std::sync::OnceLock<ToolSpec> = std::sync::OnceLock::new();
        SPEC.get_or_init(|| ToolSpec {
            name: "replace_value".to_owned(),
            description: "Must not execute for an invalid tool-call batch.".to_owned(),
            input_schema: serde_json::json!({"type": "object"}),
        })
    }

    fn execute(
        &self,
        _call: ToolCall,
        _cancellation: CancellationToken,
        _updates: ToolUpdates,
    ) -> BoxFuture<'_, Result<ToolOutput, ToolError>> {
        panic!("invalid tool-call batches must fail before tool execution")
    }
}

fn tool_call(id: &str, name: &str, arguments: serde_json::Value) -> ToolCall {
    ToolCall {
        id: id.to_owned(),
        name: name.to_owned(),
        arguments,
        thought_signature: None,
        namespace: None,
    }
}

fn tool_response(call: ToolCall) -> ModelResponse {
    ModelResponse {
        content: vec![AssistantContent::tool_call(call)],
        stop_reason: StopReason::ToolUse,
        usage: None,
        metadata: AssistantMetadata::default(),
    }
}

fn text_response(text: &str) -> ModelResponse {
    ModelResponse {
        content: vec![AssistantContent::text(text)],
        stop_reason: StopReason::Stop,
        usage: None,
        metadata: AssistantMetadata::default(),
    }
}
