use std::{
    collections::VecDeque,
    fs,
    num::NonZeroU32,
    sync::{Arc, Mutex},
};

use futures_util::{StreamExt, stream};
use nix::{errno::Errno, sys::signal::kill, unistd::Pid};
use renoa_agent::{
    AssistantContent, AssistantMetadata, ContentBlock, Message, Model, ModelError, ModelEvent,
    ModelEventStream, ModelRequest, ModelResponse, StopReason, ToolCall, ToolResult,
};
use renoa_agent_loop::{
    AgentCommand, AgentLoopConfig, ContextBinding, ModelBinding, build_runtime,
};
use renoa_kernel::{
    AgentId, Command, CommandId, DriveResult, EffectOutcome, EffectRecovery, Kernel,
    OperationOutcome, SessionId,
};
use renoa_local::LocalWorkspace;
use tempfile::tempdir;
use tokio_util::sync::CancellationToken;

#[tokio::test]
async fn bash_timeout_stops_the_process_tree_and_reaches_the_model_durably() {
    let directory = tempdir().expect("temporary directory");
    let workspace_root = directory.path().join("workspace");
    fs::create_dir(&workspace_root).expect("create workspace");
    let requests = Arc::new(Mutex::new(Vec::new()));
    let model = Arc::new(ScriptedModel::new(
        [
            tool_response(tool_call(
                "bash-timeout",
                "bash",
                serde_json::json!({
                    "command": concat!(
                        "printf 'before-timeout\\n'; ",
                        "(trap '' TERM; exec sleep 30) & ",
                        "child=$!; printf '%s\\n' \"$child\" > child.pid; wait \"$child\""
                    ),
                    "timeout_seconds": 1
                }),
            )),
            text_response("Recovered after the command timed out."),
        ],
        Arc::clone(&requests),
    ));
    let workspace = LocalWorkspace::open(&workspace_root).expect("open workspace");
    let runtime = build_runtime(
        AgentLoopConfig::new(
            "Run commands carefully.",
            NonZeroU32::new(2).expect("non-zero model limit"),
            NonZeroU32::new(1).expect("non-zero tool limit"),
        ),
        ContextBinding::full_history(),
        ModelBinding::new("scripted-timeout-v1", model, EffectRecovery::SafeToReplay),
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
                serde_json::to_value(AgentCommand::text("Run the bounded command."))
                    .expect("serialize command"),
            ),
        )
        .expect("submit command");

    assert_eq!(
        kernel
            .drive(session_id, &runtime)
            .await
            .expect("drive timeout turn"),
        DriveResult::Finished {
            operation_id: admission.operation_id,
            outcome: OperationOutcome::Completed,
        }
    );

    let child_pid = fs::read_to_string(workspace_root.join("child.pid"))
        .expect("read descendant pid")
        .trim()
        .parse::<i32>()
        .expect("parse descendant pid");
    assert_eq!(kill(Pid::from_raw(child_pid), None), Err(Errno::ESRCH));

    let requests = requests.lock().expect("request lock");
    assert_eq!(requests.len(), 2);
    let model_result = tool_result(&requests[1].messages[2]);
    assert_timeout_result(model_result);
    drop(requests);

    let snapshot = kernel.inspect(session_id).expect("inspect timeout turn");
    let operation = &snapshot.operations[0];
    assert_eq!(operation.effects.len(), 3);
    assert_eq!(operation.effects[1].recovery, EffectRecovery::NeverReplay);
    let Some(EffectOutcome::Success(value)) = &operation.effects[1].outcome else {
        panic!("Bash timeout did not settle as a durable tool result")
    };
    let durable_result: ToolResult =
        serde_json::from_value(value.clone()).expect("decode durable timeout result");
    assert_timeout_result(&durable_result);
}

fn assert_timeout_result(result: &ToolResult) {
    assert!(result.is_error);
    let [ContentBlock::Text { text }] = result.content.as_slice() else {
        panic!("timeout result did not contain one text block")
    };
    assert!(text.contains("before-timeout"));
    assert!(text.contains("timed out after 1 second"));
    assert!(text.contains("partial changes"));
}

fn tool_result(message: &Message) -> &ToolResult {
    let Message::Tool { result } = message else {
        panic!("expected a tool result message")
    };
    result
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
