use std::{num::NonZeroU32, sync::Arc};

use futures_util::{StreamExt, stream};
use renoa_agent::{
    AssistantContent, AssistantMetadata, ContentBlock, Model, ModelError, ModelEvent,
    ModelEventStream, ModelRequest, ModelResponse, StopReason, TokenUsage, ToolCall,
};
use renoa_harness::{Harness, OperationRequest, RequestId, RuntimeProfile, SessionId};
use tempfile::tempdir;
use tokio_util::sync::CancellationToken;

#[tokio::test]
async fn failed_work_keeps_complete_provider_usage_observable() {
    let directory = tempdir().expect("temporary directory");
    let usage = TokenUsage {
        input: 11,
        output: 3,
        cache_read: 7,
        cache_write: 0,
    };
    let model = Arc::new(FixedModel(ModelResponse {
        content: vec![AssistantContent::tool_call(ToolCall {
            id: "unexpected".to_owned(),
            name: "bash".to_owned(),
            arguments: serde_json::json!({"command": "true"}),
            thought_signature: None,
            namespace: None,
        })],
        stop_reason: StopReason::ToolUse,
        usage: Some(usage),
        metadata: AssistantMetadata::default(),
    }));
    let profile = RuntimeProfile::new(
        "model-only-v1",
        model,
        "Do not call tools.",
        NonZeroU32::new(1).expect("non-zero attempt limit"),
    );
    let harness = Harness::open(directory.path().join("harness.sqlite3")).expect("open harness");
    let session_id = SessionId::new();
    harness
        .create_standalone_session(session_id)
        .await
        .expect("create session");
    harness
        .admit_standalone(
            session_id,
            OperationRequest::new(RequestId::new(), vec![ContentBlock::text("finish")]),
        )
        .await
        .expect("admit operation");

    harness
        .run_next(session_id, &profile)
        .await
        .expect("settle rejected response");
    let snapshot = harness.inspect(session_id).await.expect("inspect session");

    let observed = snapshot.operations[0].model_usage;
    assert_eq!(observed.known, Some(usage));
    assert_eq!(observed.attempts, 1);
    assert_eq!(observed.attempts_without_usage, 0);
    assert_eq!(observed.outcome_unknown_attempts, 0);
}

#[tokio::test]
async fn uncertain_provider_work_is_not_reported_as_zero_usage() {
    let directory = tempdir().expect("temporary directory");
    let profile = RuntimeProfile::new(
        "model-only-v1",
        Arc::new(ErrorModel),
        "Answer precisely.",
        NonZeroU32::new(1).expect("non-zero attempt limit"),
    );
    let harness = Harness::open(directory.path().join("harness.sqlite3")).expect("open harness");
    let session_id = SessionId::new();
    harness
        .create_standalone_session(session_id)
        .await
        .expect("create session");
    harness
        .admit_standalone(
            session_id,
            OperationRequest::new(RequestId::new(), vec![ContentBlock::text("finish")]),
        )
        .await
        .expect("admit operation");

    harness
        .run_next(session_id, &profile)
        .await
        .expect("settle uncertain attempt");
    let observed = harness
        .inspect(session_id)
        .await
        .expect("inspect session")
        .operations[0]
        .model_usage;

    assert_eq!(observed.known, None);
    assert_eq!(observed.attempts, 1);
    assert_eq!(observed.attempts_without_usage, 1);
    assert_eq!(observed.outcome_unknown_attempts, 1);
}

struct FixedModel(ModelResponse);

impl Model for FixedModel {
    fn stream(
        &self,
        _request: ModelRequest,
        _cancellation: CancellationToken,
    ) -> ModelEventStream<'_> {
        stream::once(async {
            Ok(ModelEvent::Completed {
                response: self.0.clone(),
            })
        })
        .boxed()
    }
}

struct ErrorModel;

impl Model for ErrorModel {
    fn stream(
        &self,
        _request: ModelRequest,
        _cancellation: CancellationToken,
    ) -> ModelEventStream<'_> {
        stream::once(async { Err(ModelError::new("provider connection failed")) }).boxed()
    }
}
