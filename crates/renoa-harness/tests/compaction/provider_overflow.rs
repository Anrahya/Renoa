use std::{
    num::NonZeroU32,
    sync::{Arc, Mutex},
};

use futures_util::{StreamExt, stream};
use renoa_agent::{
    ContentBlock, Model, ModelError, ModelEvent, ModelEventStream, ModelRequest, StopReason,
};
use renoa_harness::{
    CompactionPolicy, ContextSizer, Harness, OperationOutcome, OperationRequest, OperationStatus,
    RequestId, RunNext, RuntimeProfile, SessionId,
};
use tempfile::tempdir;
use tokio_util::sync::CancellationToken;

use super::{
    CheckpointAwareModel, RecordingModel, compacting_profile, response, run_prompt, usage,
    valid_summary,
};

#[tokio::test]
async fn irreducible_context_failure_is_durable_and_never_dispatches() {
    let directory = tempdir().expect("temporary directory");
    let database = directory.path().join("harness.sqlite3");
    let model = Arc::new(RecordingModel::default());
    let profile = RuntimeProfile::new(
        "irreducible-v1",
        model.clone(),
        "An oversized immutable system prompt.",
        NonZeroU32::new(1).expect("one model attempt"),
    )
    .with_compaction(policy(), Arc::new(AlwaysOversized));
    let harness = Harness::open(&database).expect("open harness");
    let session_id = SessionId::new();
    harness
        .create_standalone_session(session_id)
        .await
        .expect("create session");
    let admission = harness
        .admit_standalone(
            session_id,
            OperationRequest::new(RequestId::new(), vec![ContentBlock::text("hello")]),
        )
        .await
        .expect("admit operation");

    assert!(matches!(
        harness.run_next(session_id, &profile).await,
        Ok(RunNext::Finished {
            operation_id,
            outcome: OperationOutcome::Failed { message },
        }) if operation_id == admission.operation_id && message.contains("cannot be reduced")
    ));
    assert!(model.requests().is_empty());
    drop(harness);

    let harness = Harness::open(&database).expect("reopen harness");
    let snapshot = harness.inspect(session_id).await.expect("inspect session");
    assert_eq!(snapshot.operations[0].status, OperationStatus::Failed);
    assert!(matches!(
        snapshot.outputs[0].outcome,
        OperationOutcome::Failed { ref message } if message.contains("cannot be reduced")
    ));
    assert!(matches!(
        harness
            .run_next(session_id, &profile)
            .await
            .expect("session remains usable"),
        RunNext::Idle
    ));
}

#[tokio::test]
async fn a_known_context_overflow_compacts_instead_of_replaying_the_same_request() {
    let directory = tempdir().expect("temporary directory");
    let history_model = Arc::new(CheckpointAwareModel::default());
    let harness = Harness::open(directory.path().join("harness.sqlite3")).expect("open harness");
    let session_id = SessionId::new();
    harness
        .create_standalone_session(session_id)
        .await
        .expect("create session");
    let history_profile = compacting_profile(history_model);
    run_prompt(&harness, session_id, &history_profile, "one").await;
    run_prompt(&harness, session_id, &history_profile, "two").await;

    let model = Arc::new(OverflowThenCompactModel::default());
    let profile = RuntimeProfile::new(
        "overflow-v1",
        model.clone(),
        "Be precise.",
        NonZeroU32::new(2).expect("two normal model attempts"),
    )
    .with_compaction(policy(), Arc::new(UnderestimatingSizer));
    run_prompt(&harness, session_id, &profile, "three").await;

    let requests = model.requests();
    assert_eq!(requests.len(), 3);
    assert_eq!(requests[0].system_prompt, "Be precise.");
    assert_ne!(requests[1].system_prompt, "Be precise.");
    assert!(
        serde_json::to_string(&requests[2])
            .expect("encode final request")
            .contains("CONTEXT CHECKPOINT")
    );
    let snapshot = harness.inspect(session_id).await.expect("inspect session");
    assert_eq!(snapshot.operations[2].status, OperationStatus::Completed);
    assert_eq!(snapshot.operations[2].model_usage.attempts, 3);
    assert_eq!(
        snapshot.operations[2].model_usage.outcome_unknown_attempts,
        0
    );
}

#[tokio::test]
async fn a_compaction_context_rejection_fails_without_replaying_it() {
    let directory = tempdir().expect("temporary directory");
    let history_model = Arc::new(CheckpointAwareModel::default());
    let harness = Harness::open(directory.path().join("harness.sqlite3")).expect("open harness");
    let session_id = SessionId::new();
    harness
        .create_standalone_session(session_id)
        .await
        .expect("create session");
    let history_profile = compacting_profile(history_model);
    run_prompt(&harness, session_id, &history_profile, "one").await;
    run_prompt(&harness, session_id, &history_profile, "two").await;

    let model = Arc::new(CompactionRejectingModel::default());
    let profile = compacting_profile(model.clone());
    let admission = harness
        .admit_standalone(
            session_id,
            OperationRequest::new(RequestId::new(), vec![ContentBlock::text("three")]),
        )
        .await
        .expect("admit operation");
    assert!(matches!(
        harness.run_next(session_id, &profile).await,
        Ok(RunNext::Finished {
            operation_id,
            outcome: OperationOutcome::Failed { message },
        }) if operation_id == admission.operation_id
            && message.contains("compaction request exceeded")
    ));
    assert_eq!(model.requests.lock().expect("request lock").len(), 1);
    let operation = &harness
        .inspect(session_id)
        .await
        .expect("inspect session")
        .operations[2];
    assert_eq!(operation.status, OperationStatus::Failed);
    assert_eq!(operation.model_usage.attempts, 1);
    assert_eq!(operation.model_usage.outcome_unknown_attempts, 0);
}

fn policy() -> CompactionPolicy {
    CompactionPolicy::new(
        std::num::NonZeroU64::new(100).expect("non-zero context window"),
        20,
        std::num::NonZeroU64::new(50).expect("non-zero target"),
        std::num::NonZeroU64::new(40).expect("non-zero summary limit"),
        NonZeroU32::new(1).expect("one compaction attempt"),
    )
    .expect("valid compaction policy")
}

struct AlwaysOversized;

impl ContextSizer for AlwaysOversized {
    fn estimate_input_tokens(&self, _request: &ModelRequest) -> u64 {
        90
    }
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

#[derive(Default)]
struct OverflowThenCompactModel {
    requests: Mutex<Vec<ModelRequest>>,
}

impl OverflowThenCompactModel {
    fn requests(&self) -> Vec<ModelRequest> {
        self.requests.lock().expect("request lock").clone()
    }
}

impl Model for OverflowThenCompactModel {
    fn stream(
        &self,
        request: ModelRequest,
        _cancellation: CancellationToken,
    ) -> ModelEventStream<'_> {
        let mut requests = self.requests.lock().expect("request lock");
        let call = requests.len();
        requests.push(request);
        drop(requests);
        match call {
            0 => stream::once(async {
                Err(ModelError::context_window_exceeded(
                    "xAI rejected the prompt before inference",
                ))
            })
            .boxed(),
            1 => stream::once(async {
                Ok(ModelEvent::Completed {
                    response: response(valid_summary(), StopReason::Stop, usage(6, 3)),
                })
            })
            .boxed(),
            2 => stream::once(async {
                Ok(ModelEvent::Completed {
                    response: response("done", StopReason::Stop, usage(7, 4)),
                })
            })
            .boxed(),
            _ => panic!("unexpected model call"),
        }
    }
}

#[derive(Default)]
struct CompactionRejectingModel {
    requests: Mutex<Vec<ModelRequest>>,
}

impl Model for CompactionRejectingModel {
    fn stream(
        &self,
        request: ModelRequest,
        _cancellation: CancellationToken,
    ) -> ModelEventStream<'_> {
        self.requests.lock().expect("request lock").push(request);
        stream::once(async {
            Err(ModelError::context_window_exceeded(
                "compaction request exceeded provider context",
            ))
        })
        .boxed()
    }
}
