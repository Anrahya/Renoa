use std::{num::NonZeroU32, sync::Arc};

use renoa_agent::{
    AssistantContent, AssistantMetadata, BoxFuture, ContentBlock, Message, ModelResponse,
    StopReason, Tool, ToolCall, ToolError, ToolOutput, ToolResult, ToolSpec, ToolUpdates,
};
use tempfile::tempdir;
use tokio_util::sync::CancellationToken;

use super::support::{FixedResponseModel, NeverCalledModel, create_session, response_with_usage};
use crate::{
    CancellationId, CrashPoint, Harness, OperationOutcome, OperationRequest, RequestId, RunNext,
    RuntimeProfile, ToolBinding, ToolRecovery, drive::Settlement,
};

#[tokio::test]
async fn cancellation_after_activation_prevents_the_first_model_intent() {
    let directory = tempdir().expect("temporary directory");
    let database = directory.path().join("harness.sqlite3");
    let mut harness = Harness::open(&database).expect("open harness");
    let session_id = create_session(&harness).await;
    let admission = harness
        .admit_standalone(
            session_id,
            OperationRequest::new(RequestId::new(), vec![ContentBlock::text("keep working")]),
        )
        .await
        .expect("admit operation");
    harness.crash_at(CrashPoint::ActivationCommitted);
    let profile = RuntimeProfile::new(
        "coding-v1",
        Arc::new(NeverCalledModel),
        "Be precise.",
        NonZeroU32::new(2).expect("non-zero attempt limit"),
    );
    let task = tokio::spawn(async move { harness.run_next(session_id, &profile).await });
    assert!(task.await.expect_err("injected crash").is_panic());

    let harness = Harness::open(&database).expect("reopen harness");
    harness
        .request_standalone_cancellation(session_id, admission.operation_id, CancellationId::new())
        .await
        .expect("persist cancellation");
    assert!(matches!(
        harness
            .run_next(
                session_id,
                &RuntimeProfile::new(
                    "coding-v1",
                    Arc::new(NeverCalledModel),
                    "Be precise.",
                    NonZeroU32::new(2).expect("non-zero attempt limit"),
                ),
            )
            .await
            .expect("settle cancellation before model intent"),
        RunNext::Finished {
            operation_id,
            outcome: OperationOutcome::Cancelled { .. },
        } if operation_id == admission.operation_id
    ));
}

#[tokio::test]
async fn cancellation_of_a_planned_tool_prevents_its_intent() {
    let directory = tempdir().expect("temporary directory");
    let database = directory.path().join("harness.sqlite3");
    let call = ToolCall {
        id: "call-1".to_owned(),
        name: "bash".to_owned(),
        arguments: serde_json::json!({"command": "touch should-not-exist"}),
        thought_signature: None,
        namespace: None,
    };
    let mut harness = Harness::open(&database).expect("open harness");
    let session_id = create_session(&harness).await;
    let admission = harness
        .admit_standalone(
            session_id,
            OperationRequest::new(RequestId::new(), vec![ContentBlock::text("run it")]),
        )
        .await
        .expect("admit operation");
    harness.crash_at(CrashPoint::ToolPlanCommitted);
    let profile = tool_profile(Arc::new(FixedResponseModel(tool_response(call.clone()))));
    let task = tokio::spawn(async move { harness.run_next(session_id, &profile).await });
    assert!(task.await.expect_err("injected crash").is_panic());

    let harness = Harness::open(&database).expect("reopen harness");
    harness
        .request_standalone_cancellation(session_id, admission.operation_id, CancellationId::new())
        .await
        .expect("persist cancellation");
    assert!(matches!(
        harness
            .run_next(session_id, &tool_profile(Arc::new(NeverCalledModel)))
            .await
            .expect("cancel planned tool"),
        RunNext::Finished {
            operation_id,
            outcome: OperationOutcome::Cancelled { .. },
        } if operation_id == admission.operation_id
    ));
    assert!(matches!(
        harness
            .inspect(session_id)
            .await
            .expect("inspect cancelled operation")
            .messages
            .last(),
        Some(Message::Tool { result }) if result.call_id == call.id && result.is_error
    ));
}

#[tokio::test]
async fn cancellation_committed_before_model_settlement_wins() {
    let directory = tempdir().expect("temporary directory");
    let database = directory.path().join("harness.sqlite3");
    let harness = Harness::open(&database).expect("open harness");
    let session_id = create_session(&harness).await;
    let admission = harness
        .admit_standalone(
            session_id,
            OperationRequest::new(RequestId::new(), vec![ContentBlock::text("keep working")]),
        )
        .await
        .expect("admit operation");
    let lease = harness.begin_run(session_id).expect("own session");
    let profile = RuntimeProfile::new(
        "coding-v1",
        Arc::new(NeverCalledModel),
        "Be precise.",
        NonZeroU32::new(2).expect("non-zero attempt limit"),
    );
    harness
        .store
        .activate(&lease, session_id, profile.frozen())
        .await
        .expect("activate")
        .expect("active operation");
    let crate::drive::ModelStart::Invoke(intent) = harness
        .store
        .begin_model_attempt(&lease, admission.operation_id, None)
        .await
        .expect("persist model intent")
    else {
        panic!("uncancelled operation must create a model intent");
    };
    harness
        .request_standalone_cancellation(session_id, admission.operation_id, CancellationId::new())
        .await
        .expect("persist cancellation");

    assert!(matches!(
        harness
            .store
            .settle_model(&lease, *intent, response_with_usage())
            .await
            .expect("settle completed response after cancellation"),
        Settlement::Applied(OperationOutcome::Cancelled { .. })
    ));
    assert_eq!(
        harness
            .inspect(session_id)
            .await
            .expect("inspect cancelled operation")
            .messages,
        vec![Message::user_text("keep working")]
    );
}

#[tokio::test]
async fn cancellation_committed_before_invalid_model_rejection_wins() {
    let directory = tempdir().expect("temporary directory");
    let database = directory.path().join("harness.sqlite3");
    let harness = Harness::open(&database).expect("open harness");
    let session_id = create_session(&harness).await;
    let admission = harness
        .admit_standalone(
            session_id,
            OperationRequest::new(RequestId::new(), vec![ContentBlock::text("keep working")]),
        )
        .await
        .expect("admit operation");
    let lease = harness.begin_run(session_id).expect("own session");
    let profile = RuntimeProfile::new(
        "coding-v1",
        Arc::new(NeverCalledModel),
        "Be precise.",
        NonZeroU32::new(2).expect("non-zero attempt limit"),
    );
    harness
        .store
        .activate(&lease, session_id, profile.frozen())
        .await
        .expect("activate")
        .expect("active operation");
    let crate::drive::ModelStart::Invoke(intent) = harness
        .store
        .begin_model_attempt(&lease, admission.operation_id, None)
        .await
        .expect("persist model intent")
    else {
        panic!("uncancelled operation must create a model intent");
    };
    harness
        .request_standalone_cancellation(session_id, admission.operation_id, CancellationId::new())
        .await
        .expect("persist cancellation");

    assert!(matches!(
        harness
            .store
            .reject_model_response(&lease, *intent, None, "invalid response".to_owned())
            .await
            .expect("reject invalid response after cancellation"),
        Settlement::Applied(OperationOutcome::Cancelled { .. })
    ));
    assert_eq!(
        harness
            .inspect(session_id)
            .await
            .expect("inspect cancelled operation")
            .messages,
        vec![Message::user_text("keep working")]
    );
}

#[tokio::test]
async fn cancellation_committed_before_unavailable_tool_settlement_wins() {
    let directory = tempdir().expect("temporary directory");
    let database = directory.path().join("harness.sqlite3");
    let harness = Harness::open(&database).expect("open harness");
    let session_id = create_session(&harness).await;
    let admission = harness
        .admit_standalone(
            session_id,
            OperationRequest::new(RequestId::new(), vec![ContentBlock::text("run it")]),
        )
        .await
        .expect("admit operation");
    let call = ToolCall {
        id: "call-1".to_owned(),
        name: "missing".to_owned(),
        arguments: serde_json::json!({}),
        thought_signature: None,
        namespace: None,
    };
    let profile = tool_profile(Arc::new(NeverCalledModel));
    let lease = harness.begin_run(session_id).expect("own session");
    harness
        .store
        .activate(&lease, session_id, profile.frozen())
        .await
        .expect("activate")
        .expect("active operation");
    let crate::drive::ModelStart::Invoke(intent) = harness
        .store
        .begin_model_attempt(&lease, admission.operation_id, None)
        .await
        .expect("persist model intent")
    else {
        panic!("uncancelled operation must create a model intent");
    };
    assert!(matches!(
        harness
            .store
            .settle_model(&lease, *intent, tool_response(call.clone()))
            .await
            .expect("commit unknown tool plan"),
        Settlement::Continue(_)
    ));
    let planned = harness
        .store
        .load_planned_tool(&lease, admission.operation_id)
        .await
        .expect("load unknown tool");
    assert!(planned.frozen_tool.is_none());
    harness
        .request_standalone_cancellation(session_id, admission.operation_id, CancellationId::new())
        .await
        .expect("persist cancellation");
    let unavailable = ToolResult {
        call_id: call.id.clone(),
        name: call.name.clone(),
        content: vec![ContentBlock::text("tool is unavailable")],
        details: None,
        is_error: true,
    };

    assert!(matches!(
        harness
            .store
            .settle_unavailable_tool(&lease, planned, unavailable)
            .await
            .expect("settle unknown tool after cancellation"),
        crate::drive::ToolSettlement::Finished(OperationOutcome::Cancelled { .. })
    ));
    assert!(matches!(
        harness
            .inspect(session_id)
            .await
            .expect("inspect cancelled operation")
            .messages
            .last(),
        Some(Message::Tool { result })
            if result.call_id == call.id
                && result.content == vec![ContentBlock::text(
                    "Tool call was not executed because the operation was cancelled."
                )]
    ));
}

#[tokio::test]
async fn cancellation_after_a_tool_completed_preserves_its_known_result() {
    let directory = tempdir().expect("temporary directory");
    let database = directory.path().join("harness.sqlite3");
    let harness = Harness::open(&database).expect("open harness");
    let session_id = create_session(&harness).await;
    let admission = harness
        .admit_standalone(
            session_id,
            OperationRequest::new(RequestId::new(), vec![ContentBlock::text("run it")]),
        )
        .await
        .expect("admit operation");
    let call = ToolCall {
        id: "call-1".to_owned(),
        name: "bash".to_owned(),
        arguments: serde_json::json!({"command": "printf done"}),
        thought_signature: None,
        namespace: None,
    };
    let profile = tool_profile(Arc::new(NeverCalledModel));
    let lease = harness.begin_run(session_id).expect("own session");
    harness
        .store
        .activate(&lease, session_id, profile.frozen())
        .await
        .expect("activate")
        .expect("active operation");
    let crate::drive::ModelStart::Invoke(model_intent) = harness
        .store
        .begin_model_attempt(&lease, admission.operation_id, None)
        .await
        .expect("persist model intent")
    else {
        panic!("uncancelled operation must create a model intent");
    };
    harness
        .store
        .settle_model(&lease, *model_intent, tool_response(call.clone()))
        .await
        .expect("commit tool plan");
    let planned = harness
        .store
        .load_planned_tool(&lease, admission.operation_id)
        .await
        .expect("load planned tool");
    let crate::drive::ToolStart::Invoke(tool_intent) = harness
        .store
        .begin_tool_intent(&lease, planned)
        .await
        .expect("persist tool intent")
    else {
        panic!("uncancelled operation must create a tool intent");
    };
    harness
        .request_standalone_cancellation(session_id, admission.operation_id, CancellationId::new())
        .await
        .expect("persist cancellation after the tool completed");
    let known_result = ToolResult {
        call_id: call.id.clone(),
        name: call.name.clone(),
        content: vec![ContentBlock::text("done")],
        details: None,
        is_error: false,
    };

    assert!(matches!(
        harness
            .store
            .settle_tool(&lease, *tool_intent, known_result.clone())
            .await
            .expect("settle known result after cancellation"),
        crate::drive::ToolSettlement::Finished(OperationOutcome::Cancelled { .. })
    ));
    assert!(matches!(
        harness
            .inspect(session_id)
            .await
            .expect("inspect cancelled operation")
            .messages
            .last(),
        Some(Message::Tool { result }) if result == &known_result
    ));
}

fn tool_profile(model: Arc<dyn renoa_agent::Model>) -> RuntimeProfile {
    RuntimeProfile::new(
        "coding-v1",
        model,
        "Be precise.",
        NonZeroU32::new(2).expect("non-zero attempt limit"),
    )
    .with_tools(
        vec![ToolBinding::new(
            "bash-v1",
            Arc::new(NeverCalledTool::new()),
            ToolRecovery::NeverReplay,
        )],
        NonZeroU32::new(2).expect("non-zero tool-call limit"),
    )
    .expect("valid tools")
}

fn tool_response(call: ToolCall) -> ModelResponse {
    ModelResponse {
        content: vec![AssistantContent::tool_call(call)],
        stop_reason: StopReason::ToolUse,
        usage: None,
        metadata: AssistantMetadata::default(),
    }
}

struct NeverCalledTool {
    spec: ToolSpec,
}

impl NeverCalledTool {
    fn new() -> Self {
        Self {
            spec: ToolSpec {
                name: "bash".to_owned(),
                description: "Run one command".to_owned(),
                input_schema: serde_json::json!({"type": "object"}),
            },
        }
    }
}

impl Tool for NeverCalledTool {
    fn spec(&self) -> &ToolSpec {
        &self.spec
    }

    fn execute(
        &self,
        _call: ToolCall,
        _cancellation: CancellationToken,
        _updates: ToolUpdates,
    ) -> BoxFuture<'_, Result<ToolOutput, ToolError>> {
        panic!("cancelled planned tool must not run")
    }
}
