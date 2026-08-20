use std::{
    collections::VecDeque,
    fs,
    num::NonZeroU32,
    sync::{Arc, Mutex},
    time::Duration,
};

use futures_util::{StreamExt, stream};
use renoa_agent::{
    AssistantContent, AssistantMetadata, ContentBlock, Message, Model, ModelError, ModelEvent,
    ModelEventStream, ModelRequest, ModelResponse, StopReason, ToolCall,
};
use renoa_agent_loop::{
    AgentCommand, AgentLoopConfig, ContextBinding, ModelBinding, build_runtime,
};
use renoa_kernel::{
    AgentId, CancellationId, Command, CommandId, DriveResult, EffectRecovery, EventCursor, Kernel,
    OperationOutcome, OperationStatus, SessionId,
};
use renoa_local::LocalWorkspace;
use tempfile::tempdir;
use tokio_util::sync::CancellationToken;

#[tokio::test]
async fn kernel_agent_loop_edits_a_real_local_workspace() {
    let directory = tempdir().expect("temporary directory");
    let workspace_root = directory.path().join("workspace");
    fs::create_dir(&workspace_root).expect("create workspace");
    fs::write(workspace_root.join("value.txt"), "old\n").expect("write fixture");

    let requests = Arc::new(Mutex::new(Vec::new()));
    let model = Arc::new(ScriptedModel::new(
        [
            tool_response(tool_call(
                "edit-1",
                "edit_file",
                serde_json::json!({
                    "path": "value.txt",
                    "old_text": "old\n",
                    "new_text": "new\n"
                }),
            )),
            text_response("Updated value.txt."),
        ],
        Arc::clone(&requests),
    ));
    let workspace = LocalWorkspace::open(&workspace_root).expect("open workspace");
    let runtime = build_runtime(
        AgentLoopConfig::new(
            "Edit the requested file carefully.",
            NonZeroU32::new(4).expect("non-zero model limit"),
            NonZeroU32::new(4).expect("non-zero tool limit"),
        ),
        ContextBinding::full_history(),
        ModelBinding::new("scripted-local-v1", model, EffectRecovery::SafeToReplay),
        workspace.kernel_tool_bindings(),
    )
    .expect("build kernel runtime");

    let kernel = Kernel::open(directory.path().join("kernel.sqlite3")).expect("open kernel");
    let agent_id = AgentId::new();
    let session_id = SessionId::new();
    kernel.create_agent(agent_id).expect("create agent");
    kernel
        .create_session(session_id, agent_id)
        .expect("create session");
    let command = serde_json::to_value(AgentCommand::text("Change value.txt to new."))
        .expect("serialize command");
    let admission = kernel
        .submit(session_id, Command::new(CommandId::new(), command))
        .expect("submit command");

    assert_eq!(
        kernel
            .drive(session_id, &runtime)
            .await
            .expect("drive coding turn"),
        DriveResult::Finished {
            operation_id: admission.operation_id,
            outcome: OperationOutcome::Completed,
        }
    );
    assert_eq!(
        fs::read_to_string(workspace_root.join("value.txt")).expect("read edited file"),
        "new\n"
    );

    let requests = requests.lock().expect("request lock");
    assert_eq!(requests.len(), 2);
    assert_eq!(requests[0].messages.len(), 1);
    assert_eq!(requests[1].messages.len(), 3);
    assert!(matches!(requests[1].messages[2], Message::Tool { .. }));
    drop(requests);

    let page = kernel
        .events_after(session_id, EventCursor::START)
        .expect("read durable transcript");
    assert_eq!(page.events.len(), 4);
    let snapshot = kernel.inspect(session_id).expect("inspect operation");
    assert_eq!(snapshot.operations[0].effects.len(), 3);
    assert_eq!(
        snapshot.operations[0].effects[1].recovery,
        EffectRecovery::NeverReplay
    );
}

#[tokio::test]
async fn kernel_agent_loop_routes_find_and_grep_results_back_to_the_model() {
    let directory = tempdir().expect("temporary directory");
    let workspace_root = directory.path().join("workspace");
    fs::create_dir_all(workspace_root.join("src")).expect("create source directory");
    fs::write(
        workspace_root.join("src/lib.rs"),
        "pub fn answer() -> u8 { 42 } // needle\n",
    )
    .expect("write source fixture");

    let requests = Arc::new(Mutex::new(Vec::new()));
    let model = Arc::new(ScriptedModel::new(
        [
            tool_response(tool_call(
                "find-1",
                "find",
                serde_json::json!({ "pattern": "*.rs" }),
            )),
            tool_response(tool_call(
                "grep-1",
                "grep",
                serde_json::json!({ "pattern": "needle", "glob": "*.rs" }),
            )),
            text_response("Found the matching Rust source."),
        ],
        Arc::clone(&requests),
    ));
    let workspace = LocalWorkspace::open(&workspace_root).expect("open workspace");
    let runtime = build_runtime(
        AgentLoopConfig::new(
            "Inspect the workspace carefully.",
            NonZeroU32::new(4).expect("non-zero model limit"),
            NonZeroU32::new(2).expect("non-zero tool limit"),
        ),
        ContextBinding::full_history(),
        ModelBinding::new("scripted-search-v1", model, EffectRecovery::SafeToReplay),
        workspace.kernel_tool_bindings(),
    )
    .expect("build kernel runtime");
    let kernel = Kernel::open(directory.path().join("kernel.sqlite3")).expect("open kernel");
    let agent_id = AgentId::new();
    let session_id = SessionId::new();
    kernel.create_agent(agent_id).expect("create agent");
    kernel
        .create_session(session_id, agent_id)
        .expect("create session");
    let admission = kernel
        .submit(
            session_id,
            Command::new(
                CommandId::new(),
                serde_json::to_value(AgentCommand::text("Find Rust files containing needle."))
                    .expect("serialize command"),
            ),
        )
        .expect("submit command");

    assert_eq!(
        kernel
            .drive(session_id, &runtime)
            .await
            .expect("drive search turn"),
        DriveResult::Finished {
            operation_id: admission.operation_id,
            outcome: OperationOutcome::Completed,
        }
    );

    let requests = requests.lock().expect("request lock");
    assert_eq!(requests.len(), 3);
    assert_eq!(tool_result_text(&requests[1].messages[2]), "src/lib.rs\n");
    assert_eq!(
        tool_result_text(&requests[2].messages[4]),
        "src/lib.rs:1:pub fn answer() -> u8 { 42 } // needle\n"
    );
    drop(requests);

    let snapshot = kernel.inspect(session_id).expect("inspect search turn");
    assert_eq!(snapshot.operations[0].effects.len(), 5);
    assert_eq!(
        snapshot.operations[0].effects[1].recovery,
        EffectRecovery::SafeToReplay
    );
    assert_eq!(
        snapshot.operations[0].effects[3].recovery,
        EffectRecovery::SafeToReplay
    );
}

#[tokio::test]
async fn kernel_cancellation_stops_real_bash_and_balances_the_next_model_context() {
    let directory = tempdir().expect("temporary directory");
    let workspace_root = directory.path().join("workspace");
    fs::create_dir(&workspace_root).expect("create workspace");
    let requests = Arc::new(Mutex::new(Vec::new()));
    let workspace = LocalWorkspace::open(&workspace_root).expect("open workspace");
    let runtime = Arc::new(bash_cancellation_runtime(&workspace, Arc::clone(&requests)));
    let kernel =
        Arc::new(Kernel::open(directory.path().join("kernel.sqlite3")).expect("open kernel"));
    let agent_id = AgentId::new();
    let session_id = SessionId::new();
    kernel.create_agent(agent_id).expect("create agent");
    kernel
        .create_session(session_id, agent_id)
        .expect("create session");
    let first = kernel
        .submit(
            session_id,
            Command::new(
                CommandId::new(),
                serde_json::to_value(AgentCommand::text("Run the long command."))
                    .expect("serialize command"),
            ),
        )
        .expect("submit command");
    let runner = Arc::clone(&kernel);
    let driven_runtime = Arc::clone(&runtime);
    let drive =
        tokio::spawn(async move { runner.drive(session_id, driven_runtime.as_ref()).await });
    wait_for_path(&workspace_root.join("started.txt")).await;

    kernel
        .request_cancellation(session_id, first.operation_id, CancellationId::new())
        .expect("request cancellation");
    assert_eq!(
        drive
            .await
            .expect("join driver")
            .expect("settle cancellation"),
        DriveResult::Finished {
            operation_id: first.operation_id,
            outcome: OperationOutcome::Cancelled,
        }
    );
    tokio::time::sleep(Duration::from_millis(1_200)).await;
    assert!(
        !workspace_root.join("leaked.txt").exists(),
        "a Bash child survived cancellation settlement"
    );
    assert_eq!(
        kernel
            .inspect(session_id)
            .expect("inspect cancellation")
            .operations[0]
            .status,
        OperationStatus::Cancelled
    );

    let second = kernel
        .submit(
            session_id,
            Command::new(
                CommandId::new(),
                serde_json::to_value(AgentCommand::text("Continue.")).expect("serialize command"),
            ),
        )
        .expect("submit continuation");
    assert_eq!(
        kernel
            .drive(session_id, runtime.as_ref())
            .await
            .expect("drive continuation"),
        DriveResult::Finished {
            operation_id: second.operation_id,
            outcome: OperationOutcome::Completed,
        }
    );
    let requests = requests.lock().expect("request lock");
    assert_eq!(requests.len(), 2);
    assert_eq!(requests[1].messages.len(), 4);
    let Message::Tool { result } = &requests[1].messages[2] else {
        panic!("cancelled tool result was not carried into the next model request")
    };
    assert_eq!(result.call_id, "bash-cancel");
    assert!(result.is_error);
    assert!(matches!(
        result.content.as_slice(),
        [ContentBlock::Text { text }] if text.contains("cancelled")
    ));
}

async fn wait_for_path(path: &std::path::Path) {
    tokio::time::timeout(Duration::from_secs(2), async {
        while !path.exists() {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("command did not start");
}

fn bash_cancellation_runtime(
    workspace: &LocalWorkspace,
    requests: Arc<Mutex<Vec<ModelRequest>>>,
) -> renoa_kernel::Runtime {
    let model = Arc::new(ScriptedModel::new(
        [
            tool_response(tool_call(
                "bash-cancel",
                "bash",
                serde_json::json!({
                    "command": "echo started > started.txt; (sleep 1; echo leaked > leaked.txt) & wait"
                }),
            )),
            text_response("Continued after the cancelled command."),
        ],
        requests,
    ));
    build_runtime(
        AgentLoopConfig::new(
            "Run commands carefully.",
            NonZeroU32::new(3).expect("non-zero model limit"),
            NonZeroU32::new(2).expect("non-zero tool limit"),
        ),
        ContextBinding::full_history(),
        ModelBinding::new("scripted-local-v1", model, EffectRecovery::SafeToReplay),
        workspace.kernel_tool_bindings(),
    )
    .expect("build kernel runtime")
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

fn tool_result_text(message: &Message) -> &str {
    let Message::Tool { result } = message else {
        panic!("expected a tool result message")
    };
    let [ContentBlock::Text { text }] = result.content.as_slice() else {
        panic!("tool result did not contain exactly one text block")
    };
    text
}
