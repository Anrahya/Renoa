use std::{
    num::NonZeroU32,
    sync::{
        Arc, Mutex,
        atomic::{AtomicUsize, Ordering},
    },
};

use renoa_agent::{
    AssistantContent, AssistantMetadata, BoxFuture, ContentBlock, Message, ModelResponse,
    StopReason, Tool, ToolCall, ToolError, ToolOutput, ToolResult, ToolSpec, ToolUpdates,
};
use tempfile::tempdir;
use tokio_util::sync::CancellationToken;

use super::support::{FixedResponseModel, NeverCalledModel, RecordingModel, create_session};
use crate::{
    CrashPoint, Harness, HarnessError, OperationRequest, OperationStatus, RequestId,
    RuntimeProfile, ToolBinding, ToolRecovery,
    drive::{Settlement, ToolPendingRecovery, ToolSettlement},
};

#[tokio::test]
async fn a_stale_tool_settlement_cannot_commit_after_recovery_rotates_its_token() {
    let directory = tempdir().expect("temporary directory");
    let database = directory.path().join("harness.sqlite3");
    let harness = Harness::open(&database).expect("open harness");
    let session_id = create_session(&harness).await;
    let admission = harness
        .admit_standalone(
            session_id,
            OperationRequest::new(RequestId::new(), vec![ContentBlock::text("inspect it")]),
        )
        .await
        .expect("admit operation");
    let call = tool_call();
    let tool = Arc::new(CountingTool::new(tool_spec("Read one file")));
    let profile = profile(Arc::new(NeverCalledModel), tool, ToolRecovery::SafeToReplay);
    let lease = harness.begin_run(session_id).expect("own session");
    harness
        .store
        .activate(&lease, session_id, profile.frozen())
        .await
        .expect("activate")
        .expect("active operation");
    let crate::drive::ModelStart::Invoke(model_intent) = harness
        .store
        .begin_model_attempt(&lease, admission.operation_id)
        .await
        .expect("persist model intent")
    else {
        panic!("uncancelled operation must create a model intent");
    };
    assert!(matches!(
        harness
            .store
            .settle_model(&lease, model_intent, tool_response(call.clone()))
            .await
            .expect("commit tool plan"),
        Settlement::Continue(_)
    ));
    let planned = harness
        .store
        .load_planned_tool(&lease, admission.operation_id)
        .await
        .expect("load planned tool");
    let crate::drive::ToolStart::Invoke(stale) = harness
        .store
        .begin_tool_intent(&lease, planned)
        .await
        .expect("persist first intent")
    else {
        panic!("uncancelled operation must create a tool intent");
    };
    let ToolPendingRecovery::Retry(current) = harness
        .store
        .recover_tool_attempt(&lease, admission.operation_id)
        .await
        .expect("rotate recovery token")
    else {
        panic!("safe tool must be replayable");
    };
    let result = successful_result(&call);

    assert!(matches!(
        harness
            .store
            .settle_tool(&lease, *stale, result.clone())
            .await
            .expect("reject stale result"),
        ToolSettlement::Stale
    ));
    assert!(matches!(
        harness
            .store
            .settle_tool(&lease, *current, result)
            .await
            .expect("settle current result"),
        ToolSettlement::Continue(_)
    ));
    assert_eq!(
        harness
            .store
            .count_tool_calls(admission.operation_id)
            .expect("count transient calls"),
        0
    );
    assert_eq!(
        harness
            .inspect(session_id)
            .await
            .expect("inspect session")
            .messages
            .iter()
            .filter(|message| matches!(message, Message::Tool { .. }))
            .count(),
        1
    );
}

#[tokio::test]
async fn a_changed_tool_binding_fails_before_intent_and_the_exact_binding_can_resume() {
    let directory = tempdir().expect("temporary directory");
    let database = directory.path().join("harness.sqlite3");
    let call = tool_call();
    let mut harness = Harness::open(&database).expect("open harness");
    let session_id = create_session(&harness).await;
    harness
        .admit_standalone(
            session_id,
            OperationRequest::new(RequestId::new(), vec![ContentBlock::text("inspect it")]),
        )
        .await
        .expect("admit operation");
    harness.crash_at(CrashPoint::ToolPlanCommitted);
    let original = Arc::new(CountingTool::new(tool_spec("Read one file")));
    let initial_profile = profile(
        Arc::new(FixedResponseModel(tool_response(call.clone()))),
        original,
        ToolRecovery::NeverReplay,
    );
    let task = tokio::spawn(async move { harness.run_next(session_id, &initial_profile).await });
    assert!(task.await.expect_err("injected crash").is_panic());

    let harness = Harness::open(&database).expect("reopen harness");
    let changed = Arc::new(CountingTool::new(tool_spec("Changed description")));
    let error = harness
        .run_next(
            session_id,
            &profile(
                Arc::new(NeverCalledModel),
                changed.clone(),
                ToolRecovery::NeverReplay,
            ),
        )
        .await
        .expect_err("changed binding must fail closed");
    assert_eq!(
        error,
        HarnessError::ToolBindingUnavailable {
            name: "read_file".to_owned(),
            revision: "coding-v1".to_owned(),
        }
    );
    assert_eq!(changed.call_count(), 0);
    assert_eq!(
        harness
            .inspect(session_id)
            .await
            .expect("inspect paused session")
            .operations[0]
            .status,
        OperationStatus::Running
    );

    let changed_recovery = Arc::new(CountingTool::new(tool_spec("Read one file")));
    let error = harness
        .run_next(
            session_id,
            &profile(
                Arc::new(NeverCalledModel),
                changed_recovery.clone(),
                ToolRecovery::SafeToReplay,
            ),
        )
        .await
        .expect_err("changed recovery policy must fail closed");
    assert_eq!(
        error,
        HarnessError::ToolBindingUnavailable {
            name: "read_file".to_owned(),
            revision: "coding-v1".to_owned(),
        }
    );
    assert_eq!(changed_recovery.call_count(), 0);

    assert_changed_binding_identity_fails(&harness, session_id).await;

    let exact = Arc::new(CountingTool::new(tool_spec("Read one file")));
    harness
        .run_next(
            session_id,
            &profile(
                Arc::new(RecordingModel::default()),
                exact.clone(),
                ToolRecovery::NeverReplay,
            ),
        )
        .await
        .expect("resume exact frozen binding");
    assert_eq!(exact.calls(), vec![call]);
}

async fn assert_changed_binding_identity_fails(harness: &Harness, session_id: crate::SessionId) {
    let changed = Arc::new(CountingTool::new(tool_spec("Read one file")));
    let error = harness
        .run_next(
            session_id,
            &profile_with_binding_id(
                Arc::new(NeverCalledModel),
                changed.clone(),
                ToolRecovery::NeverReplay,
                "read-file-v2",
            ),
        )
        .await
        .expect_err("changed binding identity must fail closed");
    assert_eq!(
        error,
        HarnessError::ToolBindingUnavailable {
            name: "read_file".to_owned(),
            revision: "coding-v1".to_owned(),
        }
    );
    assert_eq!(changed.call_count(), 0);
}

#[tokio::test]
async fn corrupted_replay_metadata_cannot_replay_an_unsafe_tool() {
    let directory = tempdir().expect("temporary directory");
    let database = directory.path().join("harness.sqlite3");
    let mut harness = Harness::open(&database).expect("open harness");
    let session_id = create_session(&harness).await;
    let admission = harness
        .admit_standalone(
            session_id,
            OperationRequest::new(RequestId::new(), vec![ContentBlock::text("inspect it")]),
        )
        .await
        .expect("admit operation");
    harness.crash_at(CrashPoint::ToolIntentCommitted);
    let original = Arc::new(CountingTool::new(tool_spec("Read one file")));
    let initial_profile = profile(
        Arc::new(FixedResponseModel(tool_response(tool_call()))),
        original,
        ToolRecovery::NeverReplay,
    );
    let task = tokio::spawn(async move { harness.run_next(session_id, &initial_profile).await });
    assert!(task.await.expect_err("injected crash").is_panic());

    let harness = Harness::open(&database).expect("reopen harness");
    corrupt_pending_recovery(&harness, admission.operation_id);
    let tool = Arc::new(CountingTool::new(tool_spec("Read one file")));
    let error = harness
        .run_next(
            session_id,
            &profile(
                Arc::new(NeverCalledModel),
                tool.clone(),
                ToolRecovery::NeverReplay,
            ),
        )
        .await
        .expect_err("corrupt recovery metadata must fail closed");

    assert_eq!(
        error,
        HarnessError::Corrupt("pending tool recovery differs from the frozen profile".to_owned())
    );
    assert_eq!(tool.call_count(), 0);
}

#[tokio::test]
async fn an_out_of_range_tool_cursor_fails_before_dispatch() {
    let directory = tempdir().expect("temporary directory");
    let database = directory.path().join("harness.sqlite3");
    let mut harness = Harness::open(&database).expect("open harness");
    let session_id = create_session(&harness).await;
    let admission = harness
        .admit_standalone(
            session_id,
            OperationRequest::new(RequestId::new(), vec![ContentBlock::text("inspect it")]),
        )
        .await
        .expect("admit operation");
    harness.crash_at(CrashPoint::ToolPlanCommitted);
    let original = Arc::new(CountingTool::new(tool_spec("Read one file")));
    let initial_profile = profile(
        Arc::new(FixedResponseModel(tool_response(tool_call()))),
        original,
        ToolRecovery::NeverReplay,
    );
    let task = tokio::spawn(async move { harness.run_next(session_id, &initial_profile).await });
    assert!(task.await.expect_err("injected crash").is_panic());

    let harness = Harness::open(&database).expect("reopen harness");
    corrupt_tool_cursor(&harness, admission.operation_id);
    let tool = Arc::new(CountingTool::new(tool_spec("Read one file")));
    let error = harness
        .run_next(
            session_id,
            &profile(
                Arc::new(NeverCalledModel),
                tool.clone(),
                ToolRecovery::NeverReplay,
            ),
        )
        .await
        .expect_err("out-of-range cursor must fail closed");

    assert_eq!(
        error,
        HarnessError::Corrupt("tool batch cursor is outside the batch".to_owned())
    );
    assert_eq!(tool.call_count(), 0);
}

fn corrupt_pending_recovery(harness: &Harness, operation_id: crate::OperationId) {
    let connection = harness
        .store
        .database()
        .connection()
        .expect("open owned database");
    let state_json = connection
        .query_row(
            "SELECT state_json FROM operations WHERE operation_id = ?1",
            [operation_id.to_string()],
            |row| row.get::<_, String>(0),
        )
        .expect("load pending state");
    let mut state = serde_json::from_str::<serde_json::Value>(&state_json)
        .expect("parse pending state as JSON");
    state["state"]["recovery"] = serde_json::Value::String("safe_to_replay".to_owned());
    connection
        .execute(
            "UPDATE operations SET state_json = ?2 WHERE operation_id = ?1",
            rusqlite::params![operation_id.to_string(), state.to_string()],
        )
        .expect("corrupt operation recovery");
    connection
        .execute(
            "UPDATE tool_calls SET recovery = 'safe_to_replay' WHERE operation_id = ?1",
            [operation_id.to_string()],
        )
        .expect("corrupt tool-call recovery");
}

fn corrupt_tool_cursor(harness: &Harness, operation_id: crate::OperationId) {
    let connection = harness
        .store
        .database()
        .connection()
        .expect("open owned database");
    let state_json = connection
        .query_row(
            "SELECT state_json FROM operations WHERE operation_id = ?1",
            [operation_id.to_string()],
            |row| row.get::<_, String>(0),
        )
        .expect("load planned state");
    let mut state = serde_json::from_str::<serde_json::Value>(&state_json)
        .expect("parse planned state as JSON");
    state["state"]["batch"]["next_index"] = serde_json::Value::from(1);
    connection
        .execute(
            "UPDATE operations SET state_json = ?2 WHERE operation_id = ?1",
            rusqlite::params![operation_id.to_string(), state.to_string()],
        )
        .expect("corrupt operation cursor");
    connection
        .execute(
            "UPDATE tool_calls SET source_index = 1 WHERE operation_id = ?1",
            [operation_id.to_string()],
        )
        .expect("move planned row outside the batch");
}

fn profile(
    model: Arc<dyn renoa_agent::Model>,
    tool: Arc<dyn Tool>,
    recovery: ToolRecovery,
) -> RuntimeProfile {
    profile_with_binding_id(model, tool, recovery, "read-file-v1")
}

fn profile_with_binding_id(
    model: Arc<dyn renoa_agent::Model>,
    tool: Arc<dyn Tool>,
    recovery: ToolRecovery,
    binding_id: &str,
) -> RuntimeProfile {
    RuntimeProfile::new(
        "coding-v1",
        model,
        "Be precise.",
        NonZeroU32::new(2).expect("non-zero attempt limit"),
    )
    .with_tools(
        vec![ToolBinding::new(binding_id, tool, recovery)],
        NonZeroU32::new(2).expect("non-zero tool-call limit"),
    )
    .expect("valid tools")
}

fn tool_spec(description: &str) -> ToolSpec {
    ToolSpec {
        name: "read_file".to_owned(),
        description: description.to_owned(),
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

fn successful_result(call: &ToolCall) -> ToolResult {
    ToolResult {
        call_id: call.id.clone(),
        name: call.name.clone(),
        content: vec![ContentBlock::text("contents")],
        details: None,
        is_error: false,
    }
}

struct CountingTool {
    spec: ToolSpec,
    calls: Mutex<Vec<ToolCall>>,
    call_count: AtomicUsize,
}

impl CountingTool {
    fn new(spec: ToolSpec) -> Self {
        Self {
            spec,
            calls: Mutex::new(Vec::new()),
            call_count: AtomicUsize::new(0),
        }
    }

    fn calls(&self) -> Vec<ToolCall> {
        self.calls.lock().expect("call lock").clone()
    }

    fn call_count(&self) -> usize {
        self.call_count.load(Ordering::SeqCst)
    }
}

impl Tool for CountingTool {
    fn spec(&self) -> &ToolSpec {
        &self.spec
    }

    fn execute(
        &self,
        call: ToolCall,
        _cancellation: CancellationToken,
        _updates: ToolUpdates,
    ) -> BoxFuture<'_, Result<ToolOutput, ToolError>> {
        self.call_count.fetch_add(1, Ordering::SeqCst);
        self.calls.lock().expect("call lock").push(call);
        Box::pin(std::future::ready(Ok(ToolOutput {
            content: vec![ContentBlock::text("contents")],
            details: None,
        })))
    }
}
