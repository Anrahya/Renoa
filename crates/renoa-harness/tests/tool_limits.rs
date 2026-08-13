use std::{num::NonZeroU32, sync::Arc};

use renoa_agent::{
    AssistantContent, AssistantMetadata, ContentBlock, Message, ModelResponse, StopReason,
    ToolOutput, ToolSpec,
};
use renoa_harness::{
    Harness, OperationOutcome, OperationRequest, OperationStatus, RequestId, RunNext,
    RuntimeProfile, SessionId, ToolBinding, ToolRecovery,
};
use tempfile::tempdir;

mod tool_support;

use tool_support::{RecordingTool, ScriptedModel, usage};

#[tokio::test]
async fn exhausted_model_budget_fails_only_after_tool_results_are_complete() {
    let directory = tempdir().expect("temporary directory");
    let database = directory.path().join("harness.sqlite3");
    let call = renoa_agent::ToolCall {
        id: "call-1".to_owned(),
        name: "read_file".to_owned(),
        arguments: serde_json::json!({"path": "src/lib.rs"}),
        thought_signature: None,
        namespace: None,
    };
    let model = Arc::new(ScriptedModel::new([ModelResponse {
        content: vec![AssistantContent::tool_call(call)],
        stop_reason: StopReason::ToolUse,
        usage: Some(usage(1, 1)),
        metadata: AssistantMetadata::default(),
    }]));
    let tool = Arc::new(RecordingTool::new(
        ToolSpec {
            name: "read_file".to_owned(),
            description: "Read one file".to_owned(),
            input_schema: serde_json::json!({"type": "object"}),
        },
        ToolOutput {
            content: vec![ContentBlock::text("contents")],
            details: None,
        },
    ));
    let profile = RuntimeProfile::new(
        "coding-v1",
        model.clone(),
        "Be precise.",
        NonZeroU32::new(1).expect("non-zero attempt limit"),
    )
    .with_tools(
        vec![ToolBinding::new(tool.clone(), ToolRecovery::SafeToReplay)],
        NonZeroU32::new(4).expect("non-zero tool-call limit"),
    )
    .expect("valid tools");
    let harness = Harness::open(&database).expect("open harness");
    let session_id = SessionId::new();
    harness
        .create_standalone_session(session_id)
        .await
        .expect("create session");
    harness
        .admit_standalone(
            session_id,
            OperationRequest::new(RequestId::new(), vec![ContentBlock::text("inspect it")]),
        )
        .await
        .expect("admit operation");

    assert!(matches!(
        harness
            .run_next(session_id, &profile)
            .await
            .expect("finish exhausted operation"),
        RunNext::Finished {
            outcome: OperationOutcome::Failed { ref message },
            ..
        } if message == "model attempt limit exhausted after tool results"
    ));
    assert_eq!(model.requests().len(), 1);
    assert_eq!(tool.calls().len(), 1);
    let snapshot = harness.inspect(session_id).await.expect("inspect session");
    assert_eq!(snapshot.operations[0].status, OperationStatus::Failed);
    assert!(matches!(
        snapshot.messages.last(),
        Some(Message::Tool { .. })
    ));
    assert_eq!(snapshot.outputs.len(), 1);
}

#[tokio::test]
async fn truncated_calls_repair_the_transcript_before_the_model_budget_failure() {
    let directory = tempdir().expect("temporary directory");
    let database = directory.path().join("harness.sqlite3");
    let call = renoa_agent::ToolCall {
        id: "call-truncated".to_owned(),
        name: "write_file".to_owned(),
        arguments: serde_json::json!({"content": "partial"}),
        thought_signature: None,
        namespace: None,
    };
    let model = Arc::new(ScriptedModel::new([ModelResponse {
        content: vec![AssistantContent::tool_call(call.clone())],
        stop_reason: StopReason::Length,
        usage: Some(usage(1, 1)),
        metadata: AssistantMetadata::default(),
    }]));
    let tool = Arc::new(RecordingTool::new(
        ToolSpec {
            name: call.name.clone(),
            description: "Write one file".to_owned(),
            input_schema: serde_json::json!({"type": "object"}),
        },
        ToolOutput {
            content: vec![],
            details: None,
        },
    ));
    let profile = RuntimeProfile::new(
        "coding-v1",
        model.clone(),
        "Be precise.",
        NonZeroU32::new(1).expect("non-zero attempt limit"),
    )
    .with_tools(
        vec![ToolBinding::new(tool.clone(), ToolRecovery::NeverReplay)],
        NonZeroU32::new(2).expect("non-zero tool-call limit"),
    )
    .expect("valid tools");
    let harness = Harness::open(&database).expect("open harness");
    let session_id = SessionId::new();
    harness
        .create_standalone_session(session_id)
        .await
        .expect("create session");
    harness
        .admit_standalone(
            session_id,
            OperationRequest::new(RequestId::new(), vec![ContentBlock::text("write it")]),
        )
        .await
        .expect("admit operation");

    assert!(matches!(
        harness
            .run_next(session_id, &profile)
            .await
            .expect("finish exhausted operation"),
        RunNext::Finished {
            outcome: OperationOutcome::Failed { ref message },
            ..
        } if message == "model attempt limit exhausted after tool results"
    ));
    assert_eq!(model.requests().len(), 1);
    assert!(tool.calls().is_empty());
    let snapshot = harness.inspect(session_id).await.expect("inspect session");
    assert!(
        matches!(snapshot.messages.last(), Some(Message::Tool { result }) if
        result.call_id == call.id && result.is_error)
    );
    assert_eq!(snapshot.outputs.len(), 1);
}

#[tokio::test]
async fn an_oversized_tool_batch_fails_before_any_call_is_published_or_run() {
    let directory = tempdir().expect("temporary directory");
    let database = directory.path().join("harness.sqlite3");
    let calls = ["call-1", "call-2"].map(|id| renoa_agent::ToolCall {
        id: id.to_owned(),
        name: "read_file".to_owned(),
        arguments: serde_json::json!({"path": id}),
        thought_signature: None,
        namespace: None,
    });
    let model = Arc::new(ScriptedModel::new([ModelResponse {
        content: calls.into_iter().map(AssistantContent::tool_call).collect(),
        stop_reason: StopReason::ToolUse,
        usage: Some(usage(2, 1)),
        metadata: AssistantMetadata::default(),
    }]));
    let tool = Arc::new(RecordingTool::new(
        ToolSpec {
            name: "read_file".to_owned(),
            description: "Read one file".to_owned(),
            input_schema: serde_json::json!({"type": "object"}),
        },
        ToolOutput {
            content: vec![ContentBlock::text("contents")],
            details: None,
        },
    ));
    let profile = RuntimeProfile::new(
        "coding-v1",
        model,
        "Be precise.",
        NonZeroU32::new(2).expect("non-zero attempt limit"),
    )
    .with_tools(
        vec![ToolBinding::new(tool.clone(), ToolRecovery::SafeToReplay)],
        NonZeroU32::new(1).expect("non-zero tool-call limit"),
    )
    .expect("valid tools");
    let harness = Harness::open(&database).expect("open harness");
    let session_id = SessionId::new();
    harness
        .create_standalone_session(session_id)
        .await
        .expect("create session");
    harness
        .admit_standalone(
            session_id,
            OperationRequest::new(RequestId::new(), vec![ContentBlock::text("inspect both")]),
        )
        .await
        .expect("admit operation");

    assert!(matches!(
        harness
            .run_next(session_id, &profile)
            .await
            .expect("reject oversized batch"),
        RunNext::Finished {
            outcome: OperationOutcome::Failed { ref message },
            ..
        } if message == "model returned 2 tool calls; the per-step limit is 1"
    ));
    assert!(tool.calls().is_empty());
    assert_eq!(
        harness
            .inspect(session_id)
            .await
            .expect("inspect session")
            .messages,
        vec![Message::user_text("inspect both")]
    );
}
