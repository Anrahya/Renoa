use std::{
    collections::VecDeque,
    num::NonZeroU32,
    path::PathBuf,
    sync::{Arc, Mutex},
};

use futures_util::{StreamExt, stream};
use renoa_agent::{
    AssistantContent, AssistantMetadata, BoxFuture, ContentBlock, Message, Model, ModelError,
    ModelEvent, ModelEventStream, ModelRequest, ModelResponse, StopReason, Tool, ToolCall,
    ToolError, ToolOutput, ToolResult, ToolSpec, ToolUpdates,
};
use renoa_agent_loop::{
    AgentCommand, AgentLoopConfig, AgentToolBinding, ContextBinding, ModelBinding, build_runtime,
};
use renoa_kernel::{
    AgentId, Command, CommandId, DriveResult, EffectRecovery, EffectStatus, EventCursor, Kernel,
    OperationOutcome, OperationStatus, SessionId,
};
use tempfile::tempdir;
use tokio_util::sync::CancellationToken;

const UNKNOWN_RESULT: &str =
    "This tool may have finished, but Renoa could not recover its result. It was not run again.";
const SKIPPED_RESULT: &str = "Tool call was not run because an earlier tool outcome is unknown.";

#[tokio::test]
async fn a_mutating_tool_crash_is_abandoned_honestly_without_replay() {
    let directory = tempdir().expect("temporary directory");
    let database = directory.path().join("kernel.sqlite3");
    let mutation = directory.path().join("external-effect.txt");
    let kernel = Arc::new(Kernel::open(&database).expect("open kernel"));
    let session_id = create_session(&kernel);
    let blocked = submit_text(&kernel, session_id, "Run both tools.");
    let queued = submit_text(&kernel, session_id, "Continue after recovery.");
    let calls = [
        tool_call("call-1", &mutation),
        tool_call("call-2", &directory.path().join("must-not-exist.txt")),
    ];
    let initial_runtime = runtime(
        Arc::new(ScriptedModel::new([tool_response(calls.clone())])),
        Arc::new(MutatingPanickingTool),
    );
    let driver = Arc::clone(&kernel);
    let task = tokio::spawn(async move { driver.drive(session_id, &initial_runtime).await });
    assert!(task.await.expect_err("tool panic").is_panic());
    assert_eq!(
        std::fs::read_to_string(&mutation).expect("read external mutation"),
        "tool changed external state"
    );
    drop(kernel);

    let model_requests = Arc::new(Mutex::new(Vec::new()));
    let kernel = Kernel::open(&database).expect("reopen kernel");
    let recovered_runtime = runtime(
        Arc::new(RecordingModel::new(
            [text_response("Recovered safely.")],
            Arc::clone(&model_requests),
        )),
        Arc::new(NeverCalledTool),
    );
    assert_eq!(
        kernel
            .drive(session_id, &recovered_runtime)
            .await
            .expect("classify interrupted tool"),
        DriveResult::Blocked {
            operation_id: blocked,
        }
    );
    assert!(
        model_requests
            .lock()
            .expect("model request lock")
            .is_empty()
    );

    let outcome = kernel
        .abandon_unknown_effect(session_id, blocked, &recovered_runtime)
        .expect("abandon unknown tool");
    assert!(matches!(
        outcome,
        OperationOutcome::Failed { ref reason }
            if reason == "effect outcome is unknown; operation was abandoned without replay"
    ));
    assert_eq!(
        kernel
            .abandon_unknown_effect(session_id, blocked, &recovered_runtime)
            .expect("retry abandonment"),
        outcome
    );

    let snapshot = kernel.inspect(session_id).expect("inspect abandonment");
    assert_eq!(snapshot.operations[0].status, OperationStatus::Failed);
    assert_eq!(
        snapshot.operations[0].effects[1].status,
        EffectStatus::OutcomeUnknown
    );
    assert_eq!(snapshot.operations[0].effects[1].outcome, None);
    let messages = messages(&kernel, session_id);
    assert_eq!(messages.len(), 4);
    assert_eq!(messages[2], error_result(&calls[0], UNKNOWN_RESULT));
    assert_eq!(messages[3], error_result(&calls[1], SKIPPED_RESULT));
    assert!(!directory.path().join("must-not-exist.txt").exists());

    assert!(matches!(
        kernel
            .drive(session_id, &recovered_runtime)
            .await
            .expect("run queued command"),
        DriveResult::Finished {
            operation_id,
            outcome: OperationOutcome::Completed,
        } if operation_id == queued
    ));
    let requests = model_requests.lock().expect("model request lock");
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0].messages.len(), 5);
    assert_eq!(
        requests[0].messages[2],
        error_result(&calls[0], UNKNOWN_RESULT)
    );
    assert_eq!(
        requests[0].messages[3],
        error_result(&calls[1], SKIPPED_RESULT)
    );
}

#[tokio::test]
async fn abandoning_an_unknown_model_does_not_fabricate_an_assistant_message() {
    let directory = tempdir().expect("temporary directory");
    let kernel = Kernel::open(directory.path().join("kernel.sqlite3")).expect("open kernel");
    let session_id = create_session(&kernel);
    let operation_id = submit_text(&kernel, session_id, "Ask the model once.");
    let runtime = runtime(Arc::new(UncertainModel), Arc::new(NeverCalledTool));

    assert!(matches!(
        kernel.drive(session_id, &runtime).await,
        Ok(DriveResult::Blocked { .. })
    ));
    kernel
        .abandon_unknown_effect(session_id, operation_id, &runtime)
        .expect("abandon unknown model");
    assert_eq!(
        messages(&kernel, session_id),
        vec![Message::user_text("Ask the model once.")]
    );
}

fn create_session(kernel: &Kernel) -> SessionId {
    let agent_id = AgentId::new();
    let session_id = SessionId::new();
    kernel.create_agent(agent_id).expect("create agent");
    kernel
        .create_session(session_id, agent_id)
        .expect("create session");
    session_id
}

fn submit_text(kernel: &Kernel, session_id: SessionId, text: &str) -> renoa_kernel::OperationId {
    let content = serde_json::to_value(AgentCommand::text(text)).expect("serialize command");
    kernel
        .submit(session_id, Command::new(CommandId::new(), content))
        .expect("submit command")
        .operation_id
}

fn runtime(model: Arc<dyn Model>, tool: Arc<dyn Tool>) -> renoa_kernel::Runtime {
    build_runtime(
        AgentLoopConfig::new(
            "Recover honestly.",
            NonZeroU32::new(4).expect("non-zero model limit"),
            NonZeroU32::new(4).expect("non-zero tool limit"),
        ),
        ContextBinding::full_history(),
        ModelBinding::new("model-v1", model, EffectRecovery::SafeToReplay),
        vec![AgentToolBinding::new(
            "mutating-tool-v1",
            tool,
            EffectRecovery::NeverReplay,
        )],
    )
    .expect("build runtime")
}

fn messages(kernel: &Kernel, session_id: SessionId) -> Vec<Message> {
    kernel
        .events_after(session_id, EventCursor::START)
        .expect("read message events")
        .events
        .into_iter()
        .map(|event| serde_json::from_value(event.payload).expect("decode message"))
        .collect()
}

fn tool_call(id: &str, path: &std::path::Path) -> ToolCall {
    ToolCall {
        id: id.to_owned(),
        name: "mutate_file".to_owned(),
        arguments: serde_json::json!({"path": path}),
        thought_signature: None,
        namespace: None,
    }
}

fn error_result(call: &ToolCall, message: &str) -> Message {
    Message::Tool {
        result: ToolResult {
            call_id: call.id.clone(),
            name: call.name.clone(),
            content: vec![ContentBlock::text(message)],
            details: None,
            is_error: true,
        },
    }
}

fn tool_response(calls: [ToolCall; 2]) -> ModelResponse {
    ModelResponse {
        content: calls.into_iter().map(AssistantContent::tool_call).collect(),
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

struct ScriptedModel {
    responses: Mutex<VecDeque<ModelResponse>>,
}

impl ScriptedModel {
    fn new(responses: impl IntoIterator<Item = ModelResponse>) -> Self {
        Self {
            responses: Mutex::new(responses.into_iter().collect()),
        }
    }
}

impl Model for ScriptedModel {
    fn stream(
        &self,
        _request: ModelRequest,
        _cancellation: CancellationToken,
    ) -> ModelEventStream<'_> {
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

struct RecordingModel {
    responses: Mutex<VecDeque<ModelResponse>>,
    requests: Arc<Mutex<Vec<ModelRequest>>>,
}

impl RecordingModel {
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

impl Model for RecordingModel {
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

struct UncertainModel;

impl Model for UncertainModel {
    fn stream(
        &self,
        _request: ModelRequest,
        _cancellation: CancellationToken,
    ) -> ModelEventStream<'_> {
        stream::once(async { Err(ModelError::new("provider reply was lost")) }).boxed()
    }
}

struct MutatingPanickingTool;

impl Tool for MutatingPanickingTool {
    fn spec(&self) -> &ToolSpec {
        tool_spec()
    }

    fn execute(
        &self,
        call: ToolCall,
        _cancellation: CancellationToken,
        _updates: ToolUpdates,
    ) -> BoxFuture<'_, Result<ToolOutput, ToolError>> {
        Box::pin(async move {
            let path: PathBuf = serde_json::from_value(call.arguments["path"].clone())
                .expect("decode mutation path");
            std::fs::write(path, "tool changed external state").expect("write external mutation");
            panic!("injected process loss after external mutation")
        })
    }
}

struct NeverCalledTool;

impl Tool for NeverCalledTool {
    fn spec(&self) -> &ToolSpec {
        tool_spec()
    }

    fn execute(
        &self,
        _call: ToolCall,
        _cancellation: CancellationToken,
        _updates: ToolUpdates,
    ) -> BoxFuture<'_, Result<ToolOutput, ToolError>> {
        panic!("tool must not be replayed")
    }
}

fn tool_spec() -> &'static ToolSpec {
    static SPEC: std::sync::OnceLock<ToolSpec> = std::sync::OnceLock::new();
    SPEC.get_or_init(|| ToolSpec {
        name: "mutate_file".to_owned(),
        description: "Mutate a test file.".to_owned(),
        input_schema: serde_json::json!({"type": "object"}),
    })
}
