use std::{
    num::{NonZeroU32, NonZeroU64},
    sync::{Arc, Mutex},
};

use futures_util::{StreamExt, stream};
use renoa_agent::{
    AssistantContent, AssistantMetadata, ContentBlock, Model, ModelError, ModelEvent,
    ModelEventStream, ModelRequest, ModelResponse, StopReason,
};
use tempfile::tempdir;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use super::support::{FixedResponseModel, NeverCalledModel, create_session};
use crate::{
    CompactionPolicy, ContextSizer, CrashPoint, Harness, OperationRequest, OperationStatus,
    RequestId, RuntimeProfile,
    checkpoint_format::summary_request,
    compaction::{CompactionAttempt, CompactionIntent, CompactionPlan, CompactionStart},
};

#[tokio::test]
async fn compaction_intent_is_durable_before_dispatch() {
    let directory = tempdir().expect("temporary directory");
    let database = directory.path().join("harness.sqlite3");
    let mut harness = Harness::open(&database).expect("open harness");
    let session_id = create_session(&harness).await;
    prepare_history(&harness, session_id).await;
    harness
        .admit_standalone(
            session_id,
            OperationRequest::new(RequestId::new(), vec![ContentBlock::text("three")]),
        )
        .await
        .expect("admit operation");
    harness.crash_at(CrashPoint::CompactionIntentCommitted);
    let profile = compaction_profile(Arc::new(NeverCalledModel));

    let task = tokio::spawn(async move { harness.run_next(session_id, &profile).await });
    assert!(task.await.expect_err("injected crash").is_panic());

    let harness = Harness::open(&database).expect("reopen harness");
    let snapshot = harness.inspect(session_id).await.expect("inspect session");
    assert_eq!(snapshot.operations[2].status, OperationStatus::Running);
    assert_eq!(snapshot.messages.len(), 5);
    let model = Arc::new(CheckpointAwareModel::default());
    harness
        .run_next(session_id, &compaction_profile(model.clone()))
        .await
        .expect("recover compaction");
    let requests = model.requests();
    assert_eq!(requests.len(), 2);
    assert_ne!(requests[0].system_prompt, "Be precise.");
    assert!(encoded(&requests[1]).contains("CONTEXT CHECKPOINT"));
    let snapshot = harness.inspect(session_id).await.expect("inspect session");
    assert_eq!(
        snapshot.operations[2].model_usage.outcome_unknown_attempts,
        1
    );
}

#[tokio::test]
async fn a_committed_context_rejection_forces_compaction_after_restart() {
    let directory = tempdir().expect("temporary directory");
    let database = directory.path().join("harness.sqlite3");
    let mut harness = Harness::open(&database).expect("open harness");
    let session_id = create_session(&harness).await;
    prepare_history(&harness, session_id).await;
    harness
        .admit_standalone(
            session_id,
            OperationRequest::new(RequestId::new(), vec![ContentBlock::text("three")]),
        )
        .await
        .expect("admit operation");
    harness.crash_at(CrashPoint::ContextOverflowCommitted);

    let task = tokio::spawn(async move {
        harness
            .run_next(
                session_id,
                &overflow_recovery_profile(Arc::new(OverflowModel)),
            )
            .await
    });
    assert!(task.await.expect_err("injected crash").is_panic());

    let harness = Harness::open(&database).expect("reopen harness");
    let model = Arc::new(CheckpointAwareModel::default());
    harness
        .run_next(session_id, &overflow_recovery_profile(model.clone()))
        .await
        .expect("recover through compaction");
    let requests = model.requests();
    assert_eq!(requests.len(), 2);
    assert_ne!(requests[0].system_prompt, "Be precise.");
    assert!(encoded(&requests[1]).contains("CONTEXT CHECKPOINT"));
    let snapshot = harness.inspect(session_id).await.expect("inspect session");
    assert_eq!(snapshot.operations[2].model_usage.attempts, 3);
    assert_eq!(
        snapshot.operations[2].model_usage.outcome_unknown_attempts,
        0
    );
}

#[tokio::test]
async fn a_completed_but_unsettled_checkpoint_is_not_partially_visible() {
    let directory = tempdir().expect("temporary directory");
    let database = directory.path().join("harness.sqlite3");
    let mut harness = Harness::open(&database).expect("open harness");
    let session_id = create_session(&harness).await;
    prepare_history(&harness, session_id).await;
    harness
        .admit_standalone(
            session_id,
            OperationRequest::new(RequestId::new(), vec![ContentBlock::text("three")]),
        )
        .await
        .expect("admit operation");
    harness.crash_at(CrashPoint::CompactionCompletedBeforeSettlement);
    let profile = compaction_profile(Arc::new(CheckpointAwareModel::default()));

    let task = tokio::spawn(async move { harness.run_next(session_id, &profile).await });
    assert!(task.await.expect_err("injected crash").is_panic());

    let harness = Harness::open(&database).expect("reopen harness");
    let before_recovery = harness.inspect(session_id).await.expect("inspect session");
    assert_eq!(
        before_recovery.operations[2].status,
        OperationStatus::Running
    );
    assert_eq!(before_recovery.messages.len(), 5);
    let model = Arc::new(CheckpointAwareModel::default());
    harness
        .run_next(session_id, &compaction_profile(model.clone()))
        .await
        .expect("recover compaction");
    let requests = model.requests();
    assert_eq!(requests.len(), 2);
    assert_ne!(requests[0].system_prompt, "Be precise.");
    assert!(encoded(&requests[1]).contains("CONTEXT CHECKPOINT"));
    let after_recovery = harness.inspect(session_id).await.expect("inspect session");
    assert_eq!(
        after_recovery.operations[2]
            .model_usage
            .outcome_unknown_attempts,
        1
    );
}

#[tokio::test]
async fn a_committed_checkpoint_is_never_sampled_again_after_restart() {
    let directory = tempdir().expect("temporary directory");
    let database = directory.path().join("harness.sqlite3");
    let mut harness = Harness::open(&database).expect("open harness");
    let session_id = create_session(&harness).await;
    prepare_history(&harness, session_id).await;
    harness
        .admit_standalone(
            session_id,
            OperationRequest::new(RequestId::new(), vec![ContentBlock::text("three")]),
        )
        .await
        .expect("admit operation");
    harness.crash_at(CrashPoint::CompactionSettlementCommitted);
    let profile = compaction_profile(Arc::new(CheckpointAwareModel::default()));

    let task = tokio::spawn(async move { harness.run_next(session_id, &profile).await });
    assert!(task.await.expect_err("injected crash").is_panic());

    let harness = Harness::open(&database).expect("reopen harness");
    let model = Arc::new(CheckpointAwareModel::default());
    harness
        .run_next(session_id, &compaction_profile(model.clone()))
        .await
        .expect("continue from committed checkpoint");
    let requests = model.requests();
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0].system_prompt, "Be precise.");
    assert!(encoded(&requests[0]).contains("CONTEXT CHECKPOINT"));
    let snapshot = harness.inspect(session_id).await.expect("inspect session");
    assert_eq!(snapshot.operations[2].model_usage.attempts, 2);
    assert_eq!(
        snapshot.operations[2].model_usage.outcome_unknown_attempts,
        0
    );
}

#[tokio::test]
async fn a_stale_compaction_token_cannot_activate_its_checkpoint() {
    let directory = tempdir().expect("temporary directory");
    let harness = Harness::open(directory.path().join("harness.sqlite3")).expect("open harness");
    let session_id = create_session(&harness).await;
    prepare_history(&harness, session_id).await;
    let admission = harness
        .admit_standalone(
            session_id,
            OperationRequest::new(RequestId::new(), vec![ContentBlock::text("three")]),
        )
        .await
        .expect("admit operation");
    let profile = compaction_profile(Arc::new(NeverCalledModel));
    let lease = harness.begin_run(session_id).expect("own session");
    harness
        .store
        .activate(&lease, session_id, profile.frozen())
        .await
        .expect("activate")
        .expect("active operation");
    lease
        .bind_operation(admission.operation_id)
        .expect("bind active operation");
    let source = harness
        .store
        .load_compaction_source(&lease, admission.operation_id)
        .await
        .expect("load source");
    let plan = CompactionPlan {
        request: summary_request(None, &source.entries[..=1]).expect("build summary request"),
        checkpoint_id: Uuid::new_v4(),
        previous_checkpoint_id: None,
        covered_through_sequence: source.entries[1].sequence,
    };
    let CompactionStart::Invoke(intent) = harness
        .store
        .begin_compaction(&lease, admission.operation_id, plan)
        .await
        .expect("begin compaction")
    else {
        panic!("uncancelled compaction must produce an intent");
    };
    let stale = duplicate_intent(&intent);
    let CompactionAttempt::Retry(current) = harness
        .store
        .record_compaction_uncertainty(&lease, *intent, "unknown outcome".to_owned())
        .await
        .expect("replace uncertain attempt")
    else {
        panic!("one remaining retry must be created");
    };

    assert!(matches!(
        harness
            .store
            .settle_compaction(&lease, stale, checkpoint_text().to_owned(), None)
            .await
            .expect("settle stale result"),
        CompactionAttempt::Stale
    ));
    assert!(matches!(
        harness
            .store
            .settle_compaction(&lease, *current, checkpoint_text().to_owned(), None)
            .await
            .expect("settle current result"),
        CompactionAttempt::Continue(_)
    ));
    drop(lease);

    let model = Arc::new(CheckpointAwareModel::default());
    harness
        .run_next(session_id, &compaction_profile(model.clone()))
        .await
        .expect("finish from current checkpoint");
    assert_eq!(model.requests().len(), 1);
    assert!(encoded(&model.requests()[0]).contains("CONTEXT CHECKPOINT"));
}

fn duplicate_intent(intent: &CompactionIntent) -> CompactionIntent {
    CompactionIntent {
        session_id: intent.session_id,
        operation_id: intent.operation_id,
        effect_id: intent.effect_id,
        settlement_token: intent.settlement_token,
        output_id: intent.output_id,
        progress: intent.progress.clone(),
        plan: CompactionPlan {
            request: intent.plan.request.clone(),
            checkpoint_id: intent.plan.checkpoint_id,
            previous_checkpoint_id: intent.plan.previous_checkpoint_id,
            covered_through_sequence: intent.plan.covered_through_sequence,
        },
    }
}

async fn prepare_history(harness: &Harness, session_id: crate::SessionId) {
    let profile = RuntimeProfile::new(
        "plain-v1",
        Arc::new(FixedResponseModel(normal_response())),
        "Be precise.",
        NonZeroU32::new(1).expect("non-zero model attempts"),
    );
    for prompt in ["one", "two"] {
        harness
            .admit_standalone(
                session_id,
                OperationRequest::new(RequestId::new(), vec![ContentBlock::text(prompt)]),
            )
            .await
            .expect("admit history");
        harness
            .run_next(session_id, &profile)
            .await
            .expect("run history");
    }
}

fn compaction_profile(model: Arc<dyn Model>) -> RuntimeProfile {
    RuntimeProfile::new(
        "compact-v1",
        model,
        "Be precise.",
        NonZeroU32::new(1).expect("non-zero model attempts"),
    )
    .with_compaction(
        CompactionPolicy::new(
            NonZeroU64::new(100).expect("non-zero context window"),
            20,
            NonZeroU64::new(50).expect("non-zero target"),
            NonZeroU64::new(40).expect("non-zero summary limit"),
            NonZeroU32::new(2).expect("non-zero compaction attempts"),
        )
        .expect("valid compaction policy"),
        Arc::new(TestSizer),
    )
}

fn overflow_recovery_profile(model: Arc<dyn Model>) -> RuntimeProfile {
    RuntimeProfile::new(
        "overflow-recovery-v1",
        model,
        "Be precise.",
        NonZeroU32::new(2).expect("two normal model attempts"),
    )
    .with_compaction(
        CompactionPolicy::new(
            NonZeroU64::new(100).expect("non-zero context window"),
            20,
            NonZeroU64::new(50).expect("non-zero target"),
            NonZeroU64::new(40).expect("non-zero summary limit"),
            NonZeroU32::new(1).expect("one compaction attempt"),
        )
        .expect("valid compaction policy"),
        Arc::new(UnderestimatingSizer),
    )
}

struct UnderestimatingSizer;

impl ContextSizer for UnderestimatingSizer {
    fn estimate_input_tokens(&self, request: &ModelRequest) -> u64 {
        if request.system_prompt == "Be precise." {
            10
        } else {
            40
        }
    }
}

struct OverflowModel;

impl Model for OverflowModel {
    fn stream(
        &self,
        _request: ModelRequest,
        _cancellation: CancellationToken,
    ) -> ModelEventStream<'_> {
        stream::once(async {
            Err(ModelError::context_window_exceeded(
                "provider rejected oversized context",
            ))
        })
        .boxed()
    }
}

struct TestSizer;

impl ContextSizer for TestSizer {
    fn estimate_input_tokens(&self, request: &ModelRequest) -> u64 {
        if request.system_prompt != "Be precise." {
            40
        } else if encoded(request).contains("CONTEXT CHECKPOINT") {
            30
        } else if request.messages.len() >= 5 {
            90
        } else {
            10
        }
    }
}

#[derive(Default)]
struct CheckpointAwareModel {
    requests: Mutex<Vec<ModelRequest>>,
}

impl CheckpointAwareModel {
    fn requests(&self) -> Vec<ModelRequest> {
        self.requests.lock().expect("request lock").clone()
    }
}

impl Model for CheckpointAwareModel {
    fn stream(
        &self,
        request: ModelRequest,
        _cancellation: CancellationToken,
    ) -> ModelEventStream<'_> {
        let compaction = request.system_prompt != "Be precise.";
        self.requests.lock().expect("request lock").push(request);
        let response = if compaction {
            checkpoint_response()
        } else {
            normal_response()
        };
        stream::once(async move { Ok(ModelEvent::Completed { response }) }).boxed()
    }
}

fn normal_response() -> ModelResponse {
    ModelResponse {
        content: vec![AssistantContent::text("done")],
        stop_reason: StopReason::Stop,
        usage: None,
        metadata: AssistantMetadata::default(),
    }
}

fn checkpoint_response() -> ModelResponse {
    ModelResponse {
        content: vec![AssistantContent::text(checkpoint_text())],
        stop_reason: StopReason::Stop,
        usage: None,
        metadata: AssistantMetadata::default(),
    }
}

fn checkpoint_text() -> &'static str {
    "## Goal and user intent\nContinue.\n\
     ## Hard constraints and preferences\nBe precise.\n\
     ## Completed work\nEarlier work is complete.\n\
     ## Current state and blockers\nNo blockers.\n\
     ## Decisions and rationale\nNo lasting decision.\n\
     ## Exact working facts\nNo exact facts.\n\
     ## Next action and unresolved questions\nAnswer the active request."
}

fn encoded(request: &ModelRequest) -> String {
    serde_json::to_string(request).expect("encode request")
}
