use std::{
    collections::VecDeque,
    fs,
    num::NonZeroU32,
    sync::{Arc, Mutex},
    time::Duration,
};

use futures_util::{StreamExt, stream};
use renoa_agent::{
    AssistantContent, AssistantMetadata, ContentBlock, Model, ModelError, ModelEvent,
    ModelEventStream, ModelRequest, ModelResponse, StopReason, ToolCall,
};
use renoa_harness::{
    CancellationId, Harness, OperationOutcome, OperationRequest, RequestId, RunNext,
    RuntimeProfile, SessionId,
};
use renoa_local::LocalWorkspace;
use tempfile::tempdir;
use tokio_util::sync::CancellationToken;

#[tokio::test]
async fn a_local_workspace_completes_a_read_edit_and_build_turn() {
    let directory = tempdir().expect("temporary directory");
    let workspace_root = directory.path().join("workspace");
    fs::create_dir(&workspace_root).expect("create workspace");
    fs::write(workspace_root.join("value.txt"), "old\n").expect("write fixture");

    let model = Arc::new(ScriptedModel::new([
        tool_response(tool_call(
            "read-1",
            "read_file",
            serde_json::json!({ "path": "value.txt" }),
        )),
        tool_response(tool_call(
            "edit-1",
            "edit_file",
            serde_json::json!({
                "path": "value.txt",
                "old_text": "old\n",
                "new_text": "new\n"
            }),
        )),
        tool_response(tool_call(
            "build-1",
            "bash",
            serde_json::json!({ "command": "test \"$(cat value.txt)\" = new" }),
        )),
        text_response("Updated the file and verified it."),
    ]));
    let workspace = LocalWorkspace::open(&workspace_root).expect("open local workspace");
    let profile = RuntimeProfile::new(
        "local-test-v1",
        model,
        "Edit carefully and verify the result.",
        NonZeroU32::new(4).expect("non-zero model limit"),
    )
    .with_tools(
        workspace.tool_bindings(),
        NonZeroU32::new(4).expect("non-zero tool limit"),
    )
    .expect("valid local tools");
    let harness = Harness::open(directory.path().join("harness.sqlite3")).expect("open harness");
    let session_id = SessionId::new();
    harness
        .create_standalone_session(session_id)
        .await
        .expect("create session");
    let admission = harness
        .admit_standalone(
            session_id,
            OperationRequest::new(
                RequestId::new(),
                vec![ContentBlock::text(
                    "Change value.txt from old to new, then verify it.",
                )],
            ),
        )
        .await
        .expect("admit operation");

    let outcome = harness
        .run_next(session_id, &profile)
        .await
        .expect("run local coding turn");

    assert_eq!(
        outcome,
        RunNext::Finished {
            operation_id: admission.operation_id,
            outcome: OperationOutcome::Completed {
                output: "Updated the file and verified it.".to_owned(),
                stop_reason: StopReason::Stop,
                usage: None,
            },
        }
    );
    assert_eq!(
        fs::read_to_string(workspace_root.join("value.txt")).expect("read edited file"),
        "new\n"
    );
}

#[tokio::test]
async fn a_local_workspace_can_create_a_new_file() {
    let directory = tempdir().expect("temporary directory");
    let workspace_root = directory.path().join("workspace");
    fs::create_dir(&workspace_root).expect("create workspace");
    let model = Arc::new(ScriptedModel::new([
        tool_response(tool_call(
            "write-1",
            "write_file",
            serde_json::json!({ "path": "created.txt", "content": "created\n" }),
        )),
        text_response("Created the file."),
    ]));
    let workspace = LocalWorkspace::open(&workspace_root).expect("open local workspace");
    let profile = RuntimeProfile::new(
        "local-test-v1",
        model,
        "Create the requested file.",
        NonZeroU32::new(2).expect("non-zero model limit"),
    )
    .with_tools(
        workspace.tool_bindings(),
        NonZeroU32::new(1).expect("non-zero tool limit"),
    )
    .expect("valid local tools");
    let harness = Harness::open(directory.path().join("harness.sqlite3")).expect("open harness");
    let session_id = SessionId::new();
    harness
        .create_standalone_session(session_id)
        .await
        .expect("create session");
    harness
        .admit_standalone(
            session_id,
            OperationRequest::new(
                RequestId::new(),
                vec![ContentBlock::text("Create created.txt.")],
            ),
        )
        .await
        .expect("admit operation");

    assert!(matches!(
        harness
            .run_next(session_id, &profile)
            .await
            .expect("run local coding turn"),
        RunNext::Finished {
            outcome: OperationOutcome::Completed { .. },
            ..
        }
    ));
    assert_eq!(
        fs::read_to_string(workspace_root.join("created.txt")).expect("read created file"),
        "created\n"
    );
}

#[cfg(unix)]
#[tokio::test]
async fn a_dangling_symlink_cannot_redirect_a_workspace_write() {
    let directory = tempdir().expect("temporary directory");
    let workspace_root = directory.path().join("workspace");
    let outside = directory.path().join("outside.txt");
    fs::create_dir(&workspace_root).expect("create workspace");
    std::os::unix::fs::symlink(&outside, workspace_root.join("link.txt"))
        .expect("create dangling symlink");
    let model = Arc::new(ScriptedModel::new([
        tool_response(tool_call(
            "write-escape",
            "write_file",
            serde_json::json!({ "path": "link.txt", "content": "escaped\n" }),
        )),
        text_response("Handled the write result."),
    ]));
    let workspace = LocalWorkspace::open(&workspace_root).expect("open local workspace");
    let profile = RuntimeProfile::new(
        "local-test-v1",
        model,
        "Never leave the workspace.",
        NonZeroU32::new(2).expect("non-zero model limit"),
    )
    .with_tools(
        workspace.tool_bindings(),
        NonZeroU32::new(1).expect("non-zero tool limit"),
    )
    .expect("valid local tools");
    let harness = Harness::open(directory.path().join("harness.sqlite3")).expect("open harness");
    let session_id = SessionId::new();
    harness
        .create_standalone_session(session_id)
        .await
        .expect("create session");
    harness
        .admit_standalone(
            session_id,
            OperationRequest::new(
                RequestId::new(),
                vec![ContentBlock::text("Write link.txt.")],
            ),
        )
        .await
        .expect("admit operation");

    harness
        .run_next(session_id, &profile)
        .await
        .expect("run local coding turn");

    assert!(
        !outside.exists(),
        "workspace write followed a dangling symlink"
    );
}

#[tokio::test]
async fn cancelling_bash_stops_its_child_processes_before_settlement() {
    let directory = tempdir().expect("temporary directory");
    let workspace_root = directory.path().join("workspace");
    fs::create_dir(&workspace_root).expect("create workspace");
    let model = Arc::new(ScriptedModel::new([tool_response(tool_call(
        "bash-cancel",
        "bash",
        serde_json::json!({
            "command": "echo started > started.txt; (sleep 1; echo leaked > leaked.txt) & exit 0"
        }),
    ))]));
    let workspace = LocalWorkspace::open(&workspace_root).expect("open local workspace");
    let profile = RuntimeProfile::new(
        "local-test-v1",
        model,
        "Run the command.",
        NonZeroU32::new(2).expect("non-zero model limit"),
    )
    .with_tools(
        workspace.tool_bindings(),
        NonZeroU32::new(1).expect("non-zero tool limit"),
    )
    .expect("valid local tools");
    let harness =
        Arc::new(Harness::open(directory.path().join("harness.sqlite3")).expect("open harness"));
    let session_id = SessionId::new();
    harness
        .create_standalone_session(session_id)
        .await
        .expect("create session");
    let admission = harness
        .admit_standalone(
            session_id,
            OperationRequest::new(
                RequestId::new(),
                vec![ContentBlock::text("Run the command.")],
            ),
        )
        .await
        .expect("admit operation");
    let driver = Arc::clone(&harness);
    let run = tokio::spawn(async move { driver.run_next(session_id, &profile).await });
    wait_for_path(&workspace_root.join("started.txt")).await;

    harness
        .request_standalone_cancellation(session_id, admission.operation_id, CancellationId::new())
        .await
        .expect("request cancellation");
    let outcome = run
        .await
        .expect("join driver")
        .expect("settle cancellation");

    assert!(matches!(
        outcome,
        RunNext::Finished {
            outcome: OperationOutcome::Cancelled { .. },
            ..
        }
    ));
    tokio::time::sleep(Duration::from_millis(1_200)).await;
    assert!(
        !workspace_root.join("leaked.txt").exists(),
        "a child process survived cancellation settlement"
    );
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
            .expect("model response lock")
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
