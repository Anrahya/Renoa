use std::{
    collections::VecDeque,
    fs,
    num::NonZeroU32,
    sync::{Arc, Mutex},
};

use futures_util::{StreamExt, stream};
use renoa_agent::{
    AssistantContent, AssistantMetadata, Message, Model, ModelError, ModelEvent, ModelEventStream,
    ModelRequest, ModelResponse, StopReason, ToolCall,
};
use renoa_agent_loop::{AgentCommand, AgentLoopConfig, ModelBinding, build_runtime};
use renoa_kernel::{
    AgentId, Command, CommandId, DriveResult, EffectRecovery, EventCursor, Kernel,
    OperationOutcome, SessionId,
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
