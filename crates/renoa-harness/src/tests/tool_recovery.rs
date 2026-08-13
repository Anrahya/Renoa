use std::{
    num::NonZeroU32,
    sync::{Arc, Mutex},
};

use renoa_agent::{
    AssistantContent, AssistantMetadata, BoxFuture, ContentBlock, Message, ModelResponse,
    StopReason, Tool, ToolCall, ToolError, ToolOutput, ToolSpec, ToolUpdates,
};
use tempfile::tempdir;
use tokio_util::sync::CancellationToken;

use super::support::{FixedResponseModel, RecordingModel, create_session};
use crate::{
    CrashPoint, Harness, OperationRequest, OperationStatus, RequestId, RunNext, RuntimeProfile,
    ToolBinding, ToolRecovery,
};

#[tokio::test]
async fn a_safe_tool_intent_replays_the_exact_call_after_restart() {
    let directory = tempdir().expect("temporary directory");
    let database = directory.path().join("harness.sqlite3");
    let call = ToolCall {
        id: "call-1".to_owned(),
        name: "read_file".to_owned(),
        arguments: serde_json::json!({"path": "src/lib.rs"}),
        thought_signature: Some("sig".to_owned()),
        namespace: Some("workspace".to_owned()),
    };
    let mut harness = Harness::open(&database).expect("open harness");
    let session_id = create_session(&harness).await;
    harness
        .admit_standalone(
            session_id,
            OperationRequest::new(RequestId::new(), vec![ContentBlock::text("inspect it")]),
        )
        .await
        .expect("admit operation");
    harness.crash_at(CrashPoint::ToolIntentCommitted);
    let never_called = Arc::new(RecordingTool::new(true));
    let crash_profile = profile(
        Arc::new(FixedResponseModel(tool_response(call.clone()))),
        never_called.clone(),
    );

    let task = tokio::spawn(async move { harness.run_next(session_id, &crash_profile).await });
    assert!(task.await.expect_err("injected crash").is_panic());
    assert!(never_called.calls().is_empty());

    let harness = Harness::open(&database).expect("reopen harness");
    let model = Arc::new(RecordingModel::default());
    let replayed = Arc::new(RecordingTool::new(false));
    harness
        .run_next(session_id, &profile(model.clone(), replayed.clone()))
        .await
        .expect("recover operation");

    assert_eq!(replayed.calls(), vec![call]);
    let requests = model.requests();
    assert_eq!(requests.len(), 1);
    assert!(matches!(
        requests[0].messages.last(),
        Some(Message::Tool { .. })
    ));
}

#[tokio::test]
async fn a_committed_plan_without_an_intent_executes_after_restart() {
    let directory = tempdir().expect("temporary directory");
    let database = directory.path().join("harness.sqlite3");
    let call = ToolCall {
        id: "call-1".to_owned(),
        name: "read_file".to_owned(),
        arguments: serde_json::json!({"path": "src/lib.rs"}),
        thought_signature: None,
        namespace: None,
    };
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
    let never_called = Arc::new(RecordingTool::new(true));
    let crash_profile = profile_with_recovery(
        Arc::new(FixedResponseModel(tool_response(call.clone()))),
        never_called.clone(),
        ToolRecovery::NeverReplay,
    );
    let task = tokio::spawn(async move { harness.run_next(session_id, &crash_profile).await });
    assert!(task.await.expect_err("injected crash").is_panic());
    assert!(never_called.calls().is_empty());

    let harness = Harness::open(&database).expect("reopen harness");
    let executed = Arc::new(RecordingTool::new(false));
    harness
        .run_next(
            session_id,
            &profile_with_recovery(
                Arc::new(RecordingModel::default()),
                executed.clone(),
                ToolRecovery::NeverReplay,
            ),
        )
        .await
        .expect("resume planned call");
    assert_eq!(executed.calls(), vec![call]);
}

#[tokio::test]
async fn a_safe_completed_but_unsettled_tool_is_replayed_once() {
    let directory = tempdir().expect("temporary directory");
    let database = directory.path().join("harness.sqlite3");
    let call = ToolCall {
        id: "call-1".to_owned(),
        name: "read_file".to_owned(),
        arguments: serde_json::json!({"path": "src/lib.rs"}),
        thought_signature: None,
        namespace: None,
    };
    let mut harness = Harness::open(&database).expect("open harness");
    let session_id = create_session(&harness).await;
    harness
        .admit_standalone(
            session_id,
            OperationRequest::new(RequestId::new(), vec![ContentBlock::text("inspect it")]),
        )
        .await
        .expect("admit operation");
    harness.crash_at(CrashPoint::ToolCompletedBeforeSettlement);
    let first = Arc::new(RecordingTool::new(false));
    let crash_profile = profile(
        Arc::new(FixedResponseModel(tool_response(call.clone()))),
        first.clone(),
    );

    let task = tokio::spawn(async move { harness.run_next(session_id, &crash_profile).await });
    assert!(task.await.expect_err("injected crash").is_panic());
    assert_eq!(first.calls(), vec![call.clone()]);

    let harness = Harness::open(&database).expect("reopen harness");
    let replayed = Arc::new(RecordingTool::new(false));
    harness
        .run_next(
            session_id,
            &profile(Arc::new(RecordingModel::default()), replayed.clone()),
        )
        .await
        .expect("recover operation");
    assert_eq!(replayed.calls(), vec![call]);
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
async fn a_settled_tool_result_is_never_executed_again() {
    let directory = tempdir().expect("temporary directory");
    let database = directory.path().join("harness.sqlite3");
    let first_call = ToolCall {
        id: "call-1".to_owned(),
        name: "read_file".to_owned(),
        arguments: serde_json::json!({"path": "src/lib.rs"}),
        thought_signature: None,
        namespace: None,
    };
    let second_call = ToolCall {
        id: "call-2".to_owned(),
        name: "read_file".to_owned(),
        arguments: serde_json::json!({"path": "src/main.rs"}),
        thought_signature: None,
        namespace: None,
    };
    let mut harness = Harness::open(&database).expect("open harness");
    let session_id = create_session(&harness).await;
    harness
        .admit_standalone(
            session_id,
            OperationRequest::new(RequestId::new(), vec![ContentBlock::text("inspect it")]),
        )
        .await
        .expect("admit operation");
    harness.crash_at(CrashPoint::ToolSettlementCommitted);
    let first = Arc::new(RecordingTool::new(false));
    let crash_profile = profile(
        Arc::new(FixedResponseModel(tool_batch_response(vec![
            first_call.clone(),
            second_call.clone(),
        ]))),
        first.clone(),
    );

    let task = tokio::spawn(async move { harness.run_next(session_id, &crash_profile).await });
    assert!(task.await.expect_err("injected crash").is_panic());
    assert_eq!(first.calls(), vec![first_call]);

    let harness = Harness::open(&database).expect("reopen harness");
    let recovered = Arc::new(RecordingTool::new(false));
    harness
        .run_next(
            session_id,
            &profile(Arc::new(RecordingModel::default()), recovered.clone()),
        )
        .await
        .expect("finish recovered operation");
    assert_eq!(recovered.calls(), vec![second_call]);
    assert_eq!(
        harness
            .inspect(session_id)
            .await
            .expect("inspect session")
            .messages
            .iter()
            .filter(|message| matches!(message, Message::Tool { .. }))
            .count(),
        2
    );
}

#[tokio::test]
async fn an_unsafe_pending_tool_blocks_without_replay() {
    let directory = tempdir().expect("temporary directory");
    let database = directory.path().join("harness.sqlite3");
    let call = ToolCall {
        id: "call-1".to_owned(),
        name: "read_file".to_owned(),
        arguments: serde_json::json!({"path": "src/lib.rs"}),
        thought_signature: None,
        namespace: None,
    };
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
    let never_called = Arc::new(RecordingTool::new(true));
    let crash_profile = profile_with_recovery(
        Arc::new(FixedResponseModel(tool_response(call))),
        never_called.clone(),
        ToolRecovery::NeverReplay,
    );

    let task = tokio::spawn(async move { harness.run_next(session_id, &crash_profile).await });
    assert!(task.await.expect_err("injected crash").is_panic());

    let harness = Harness::open(&database).expect("reopen harness");
    let recovery_profile = profile_with_recovery(
        Arc::new(super::support::NeverCalledModel),
        never_called.clone(),
        ToolRecovery::NeverReplay,
    );
    assert_eq!(
        harness
            .run_next(session_id, &recovery_profile)
            .await
            .expect("pause unknown tool"),
        RunNext::Blocked {
            operation_id: admission.operation_id,
        }
    );
    assert_eq!(
        harness
            .run_next(session_id, &recovery_profile)
            .await
            .expect("remain paused"),
        RunNext::Blocked {
            operation_id: admission.operation_id,
        }
    );
    assert!(never_called.calls().is_empty());
    assert_eq!(
        harness
            .inspect(session_id)
            .await
            .expect("inspect session")
            .operations[0]
            .status,
        OperationStatus::OutcomeUnknown
    );
}

fn profile(model: Arc<dyn renoa_agent::Model>, tool: Arc<dyn Tool>) -> RuntimeProfile {
    profile_with_recovery(model, tool, ToolRecovery::SafeToReplay)
}

fn profile_with_recovery(
    model: Arc<dyn renoa_agent::Model>,
    tool: Arc<dyn Tool>,
    recovery: ToolRecovery,
) -> RuntimeProfile {
    RuntimeProfile::new(
        "coding-v1",
        model,
        "Be precise.",
        NonZeroU32::new(3).expect("non-zero attempt limit"),
    )
    .with_tools(
        vec![ToolBinding::new("read-file-v1", tool, recovery)],
        NonZeroU32::new(4).expect("non-zero tool-call limit"),
    )
    .expect("valid tools")
}

fn tool_response(call: ToolCall) -> ModelResponse {
    tool_batch_response(vec![call])
}

fn tool_batch_response(calls: Vec<ToolCall>) -> ModelResponse {
    ModelResponse {
        content: calls.into_iter().map(AssistantContent::tool_call).collect(),
        stop_reason: StopReason::ToolUse,
        usage: None,
        metadata: AssistantMetadata::default(),
    }
}

struct RecordingTool {
    spec: ToolSpec,
    panic_on_call: bool,
    calls: Mutex<Vec<ToolCall>>,
}

impl RecordingTool {
    fn new(panic_on_call: bool) -> Self {
        Self {
            spec: ToolSpec {
                name: "read_file".to_owned(),
                description: "Read one file".to_owned(),
                input_schema: serde_json::json!({"type": "object"}),
            },
            panic_on_call,
            calls: Mutex::new(Vec::new()),
        }
    }

    fn calls(&self) -> Vec<ToolCall> {
        self.calls.lock().expect("call lock").clone()
    }
}

impl Tool for RecordingTool {
    fn spec(&self) -> &ToolSpec {
        &self.spec
    }

    fn execute(
        &self,
        call: ToolCall,
        _cancellation: CancellationToken,
        _updates: ToolUpdates,
    ) -> BoxFuture<'_, Result<ToolOutput, ToolError>> {
        assert!(!self.panic_on_call, "tool must not be dispatched");
        self.calls.lock().expect("call lock").push(call);
        Box::pin(std::future::ready(Ok(ToolOutput {
            content: vec![ContentBlock::text("contents")],
            details: None,
        })))
    }
}
