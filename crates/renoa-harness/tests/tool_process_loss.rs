use std::{num::NonZeroU32, path::PathBuf, sync::Arc, time::Duration};

use renoa_agent::{
    AssistantContent, AssistantMetadata, BoxFuture, ContentBlock, Model, ModelResponse, StopReason,
    Tool, ToolCall, ToolError, ToolOutput, ToolSpec, ToolUpdates,
};
use renoa_harness::{
    Harness, OperationRequest, OperationStatus, RequestId, RunNext, RuntimeProfile, SessionId,
    ToolBinding, ToolRecovery,
};
use tempfile::{TempDir, tempdir};
use tokio::sync::Notify;
use tokio_util::sync::CancellationToken;

mod tool_support;

use tool_support::{RecordingTool, ScriptedModel};

#[tokio::test]
async fn process_loss_during_a_safe_tool_replays_the_exact_call() {
    let interrupted = interrupt_running_tool(ToolRecovery::SafeToReplay).await;
    let model = Arc::new(ScriptedModel::new([text_response("done")]));
    let tool = Arc::new(RecordingTool::new(
        tool_spec(),
        ToolOutput {
            content: vec![ContentBlock::text("contents")],
            details: None,
        },
    ));
    let harness = Harness::open(&interrupted.database).expect("reopen harness");

    assert!(matches!(
        harness
            .run_next(
                interrupted.session_id,
                &profile(model, tool.clone(), ToolRecovery::SafeToReplay),
            )
            .await
            .expect("recover safe tool"),
        RunNext::Finished { operation_id, .. }
            if operation_id == interrupted.operation_id
    ));
    assert_eq!(tool.calls(), vec![interrupted.call]);
}

#[tokio::test]
async fn process_loss_during_an_unsafe_tool_blocks_without_replay() {
    let interrupted = interrupt_running_tool(ToolRecovery::NeverReplay).await;
    let tool = Arc::new(PanicTool { spec: tool_spec() });
    let harness = Harness::open(&interrupted.database).expect("reopen harness");

    assert_eq!(
        harness
            .run_next(
                interrupted.session_id,
                &profile(
                    Arc::new(ScriptedModel::new(Vec::<ModelResponse>::new())),
                    tool,
                    ToolRecovery::NeverReplay,
                ),
            )
            .await
            .expect("recover unsafe tool"),
        RunNext::Blocked {
            operation_id: interrupted.operation_id,
        }
    );
    assert_eq!(
        harness
            .inspect(interrupted.session_id)
            .await
            .expect("inspect blocked operation")
            .operations[0]
            .status,
        OperationStatus::OutcomeUnknown
    );
}

struct InterruptedToolRun {
    _directory: TempDir,
    database: PathBuf,
    session_id: SessionId,
    operation_id: renoa_harness::OperationId,
    call: ToolCall,
}

async fn interrupt_running_tool(recovery: ToolRecovery) -> InterruptedToolRun {
    let directory = tempdir().expect("temporary directory");
    let database = directory.path().join("harness.sqlite3");
    let harness = Arc::new(Harness::open(&database).expect("open harness"));
    let session_id = SessionId::new();
    harness
        .create_standalone_session(session_id)
        .await
        .expect("create session");
    let admission = harness
        .admit_standalone(
            session_id,
            OperationRequest::new(RequestId::new(), vec![ContentBlock::text("inspect it")]),
        )
        .await
        .expect("admit operation");
    let call = tool_call();
    let tool = Arc::new(BlockingTool::new());
    let run_profile = profile(
        Arc::new(ScriptedModel::new([tool_response(call.clone())])),
        tool.clone(),
        recovery,
    );
    let driver = Arc::clone(&harness);
    let run = tokio::spawn(async move { driver.run_next(session_id, &run_profile).await });
    tokio::time::timeout(Duration::from_secs(2), tool.started.notified())
        .await
        .expect("tool was dispatched");

    run.abort();
    assert!(
        run.await
            .expect_err("abort in-flight driver")
            .is_cancelled()
    );
    drop(harness);

    InterruptedToolRun {
        _directory: directory,
        database,
        session_id,
        operation_id: admission.operation_id,
        call,
    }
}

fn profile(model: Arc<dyn Model>, tool: Arc<dyn Tool>, recovery: ToolRecovery) -> RuntimeProfile {
    RuntimeProfile::new(
        "coding-v1",
        model,
        "Be precise.",
        NonZeroU32::new(3).expect("non-zero attempt limit"),
    )
    .with_tools(
        vec![ToolBinding::new("read-file-v1", tool, recovery)],
        NonZeroU32::new(1).expect("non-zero tool-call limit"),
    )
    .expect("valid profile")
}

fn tool_spec() -> ToolSpec {
    ToolSpec {
        name: "read_file".to_owned(),
        description: "Read one file".to_owned(),
        input_schema: serde_json::json!({"type": "object"}),
    }
}

fn tool_call() -> ToolCall {
    ToolCall {
        id: "call-1".to_owned(),
        name: "read_file".to_owned(),
        arguments: serde_json::json!({"path": "src/lib.rs"}),
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

struct BlockingTool {
    spec: ToolSpec,
    started: Notify,
}

impl BlockingTool {
    fn new() -> Self {
        Self {
            spec: tool_spec(),
            started: Notify::new(),
        }
    }
}

impl Tool for BlockingTool {
    fn spec(&self) -> &ToolSpec {
        &self.spec
    }

    fn execute(
        &self,
        _call: ToolCall,
        cancellation: CancellationToken,
        _updates: ToolUpdates,
    ) -> BoxFuture<'_, Result<ToolOutput, ToolError>> {
        Box::pin(async move {
            self.started.notify_one();
            cancellation.cancelled().await;
            Err(ToolError::new("tool stopped"))
        })
    }
}

struct PanicTool {
    spec: ToolSpec,
}

impl Tool for PanicTool {
    fn spec(&self) -> &ToolSpec {
        &self.spec
    }

    fn execute(
        &self,
        _call: ToolCall,
        _cancellation: CancellationToken,
        _updates: ToolUpdates,
    ) -> BoxFuture<'_, Result<ToolOutput, ToolError>> {
        panic!("an unsafe in-flight tool must not replay")
    }
}
