use std::{num::NonZeroU32, sync::Arc};

use renoa_agent::{
    AssistantContent, AssistantMetadata, BoxFuture, ContentBlock, Message, ModelResponse,
    StopReason, Tool, ToolCall, ToolError, ToolOutput, ToolResult, ToolSpec, ToolUpdates,
};
use tempfile::tempdir;
use tokio_util::sync::CancellationToken;

use super::support::{FixedResponseModel, NeverCalledModel, RecordingModel, create_session};
use crate::{
    CrashPoint, Harness, OperationOutcome, OperationRequest, OperationStatus, RequestId, RunNext,
    RuntimeProfile, SessionSnapshot, ToolBinding, ToolRecovery,
};

#[tokio::test]
async fn abandonment_repairs_the_batch_idempotently_and_unblocks_queued_work() {
    let directory = tempdir().expect("temporary directory");
    let database = directory.path().join("harness.sqlite3");
    let calls = ["call-1", "call-2"].map(|id| ToolCall {
        id: id.to_owned(),
        name: "read_file".to_owned(),
        arguments: serde_json::json!({"path": id}),
        thought_signature: None,
        namespace: None,
    });
    let mut harness = Harness::open(&database).expect("open harness");
    let session_id = create_session(&harness).await;
    let blocked = harness
        .admit_standalone(
            session_id,
            OperationRequest::new(RequestId::new(), vec![ContentBlock::text("inspect both")]),
        )
        .await
        .expect("admit first operation");
    let queued = harness
        .admit_standalone(
            session_id,
            OperationRequest::new(
                RequestId::new(),
                vec![ContentBlock::text("continue safely")],
            ),
        )
        .await
        .expect("admit second operation");
    harness.crash_at(CrashPoint::ToolIntentCommitted);
    let initial_profile = tool_profile(Arc::new(FixedResponseModel(ModelResponse {
        content: calls
            .iter()
            .cloned()
            .map(AssistantContent::tool_call)
            .collect(),
        stop_reason: StopReason::ToolUse,
        usage: None,
        metadata: AssistantMetadata::default(),
    })));
    let task = tokio::spawn(async move { harness.run_next(session_id, &initial_profile).await });
    assert!(task.await.expect_err("injected crash").is_panic());

    let harness = Harness::open(&database).expect("reopen harness");
    assert_eq!(
        harness
            .run_next(session_id, &tool_profile(Arc::new(NeverCalledModel)))
            .await
            .expect("pause unknown tool"),
        RunNext::Blocked {
            operation_id: blocked.operation_id,
        }
    );
    let outcome = OperationOutcome::Failed {
        message: "tool outcome is unknown; the operation was abandoned without replay".to_owned(),
    };
    assert_eq!(
        harness
            .abandon_unknown_tool(session_id, blocked.operation_id)
            .await
            .expect("abandon unknown tool"),
        outcome
    );
    assert_eq!(
        harness
            .abandon_unknown_tool(session_id, blocked.operation_id)
            .await
            .expect("retry lost abandonment reply"),
        outcome
    );
    assert_eq!(
        harness
            .store
            .count_tool_calls(blocked.operation_id)
            .expect("count transient calls"),
        0
    );
    assert_abandoned_batch(
        &harness.inspect(session_id).await.expect("inspect session"),
        &outcome,
        &calls,
    );

    let next = harness
        .run_next(session_id, &model_only_profile())
        .await
        .expect("run queued operation");
    assert!(matches!(
        next,
        RunNext::Finished { operation_id, .. } if operation_id == queued.operation_id
    ));
    let snapshot = harness
        .inspect(session_id)
        .await
        .expect("inspect final session");
    assert_eq!(snapshot.operations[1].status, OperationStatus::Completed);
    assert!(
        snapshot
            .messages
            .contains(&Message::user_text("continue safely"))
    );
}

fn assert_abandoned_batch(
    snapshot: &SessionSnapshot,
    outcome: &OperationOutcome,
    calls: &[ToolCall],
) {
    assert_eq!(snapshot.operations[0].status, OperationStatus::Failed);
    assert_eq!(&snapshot.outputs[0].outcome, outcome);
    assert_eq!(
        &snapshot.messages[2..],
        &[
            error_result(
                &calls[0],
                "Tool outcome is unknown after restart; it was not retried.",
            ),
            error_result(
                &calls[1],
                "Tool call was not executed because an earlier tool outcome is unknown.",
            ),
        ]
    );
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

fn tool_profile(model: Arc<dyn renoa_agent::Model>) -> RuntimeProfile {
    RuntimeProfile::new(
        "coding-v1",
        model,
        "Be precise.",
        NonZeroU32::new(3).expect("non-zero attempt limit"),
    )
    .with_tools(
        vec![ToolBinding::new(
            "read-file-v1",
            Arc::new(NeverExecutedTool::new()),
            ToolRecovery::NeverReplay,
        )],
        NonZeroU32::new(4).expect("non-zero tool-call limit"),
    )
    .expect("valid tools")
}

fn model_only_profile() -> RuntimeProfile {
    RuntimeProfile::new(
        "plain-v1",
        Arc::new(RecordingModel::default()),
        "Be precise.",
        NonZeroU32::new(1).expect("non-zero attempt limit"),
    )
}

struct NeverExecutedTool {
    spec: ToolSpec,
}

impl NeverExecutedTool {
    fn new() -> Self {
        Self {
            spec: ToolSpec {
                name: "read_file".to_owned(),
                description: "Read one file".to_owned(),
                input_schema: serde_json::json!({"type": "object"}),
            },
        }
    }
}

impl Tool for NeverExecutedTool {
    fn spec(&self) -> &ToolSpec {
        &self.spec
    }

    fn execute(
        &self,
        _call: ToolCall,
        _cancellation: CancellationToken,
        _updates: ToolUpdates,
    ) -> BoxFuture<'_, Result<ToolOutput, ToolError>> {
        panic!("unsafe tool must not be executed")
    }
}
