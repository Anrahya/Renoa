use std::{
    collections::VecDeque,
    num::NonZeroU32,
    sync::{Arc, Mutex},
};

use futures_util::{StreamExt, stream};
use renoa_agent::{
    AssistantContent, AssistantMetadata, ContentBlock, Model, ModelEvent, ModelEventStream,
    ModelRequest, ModelResponse, StopReason, ToolCall,
};
use renoa_harness::{
    CancellationId, Harness, OperationOutcome, OperationRequest, OperationStatus, RequestId,
    RunNext, RuntimeProfile, SessionId,
};
use tempfile::tempdir;
use tokio::{sync::Notify, time::timeout};
use tokio_util::sync::CancellationToken;

use super::{
    CheckpointAwareModel, PendingCompactionModel, compacting_profile, response, run_prompt, usage,
    valid_summary,
};

#[tokio::test]
async fn cancelling_a_dispatched_compaction_never_activates_its_summary() {
    let directory = tempdir().expect("temporary directory");
    let started = Arc::new(Notify::new());
    let pending = Arc::new(PendingCompactionModel::new(Arc::clone(&started)));
    let profile = Arc::new(compacting_profile(pending));
    let harness =
        Arc::new(Harness::open(directory.path().join("harness.sqlite3")).expect("open harness"));
    let session_id = SessionId::new();
    harness
        .create_standalone_session(session_id)
        .await
        .expect("create session");
    run_prompt(&harness, session_id, &profile, "one").await;
    run_prompt(&harness, session_id, &profile, "two").await;
    let admission = harness
        .admit_standalone(
            session_id,
            OperationRequest::new(RequestId::new(), vec![ContentBlock::text("three")]),
        )
        .await
        .expect("admit operation");
    let runner = Arc::clone(&harness);
    let runner_profile = Arc::clone(&profile);
    let run = tokio::spawn(async move { runner.run_next(session_id, &runner_profile).await });
    timeout(std::time::Duration::from_secs(2), started.notified())
        .await
        .expect("compaction starts");

    harness
        .request_standalone_cancellation(session_id, admission.operation_id, CancellationId::new())
        .await
        .expect("request cancellation");
    assert!(matches!(
        run.await.expect("driver joins").expect("driver settles"),
        RunNext::Finished {
            outcome: OperationOutcome::Cancelled { .. },
            ..
        }
    ));

    let next_model = Arc::new(CheckpointAwareModel::default());
    let next_profile = compacting_profile(next_model.clone());
    run_prompt(&harness, session_id, &next_profile, "four").await;
    let next_compactor = next_model
        .requests()
        .into_iter()
        .find(|request| request.system_prompt != "Be precise.")
        .expect("next operation compacts raw history");
    let input = serde_json::to_string(&next_compactor).expect("encode compactor request");
    assert!(!input.contains("previous_checkpoint"));
}

#[tokio::test]
async fn a_tool_shaped_summary_uses_known_usage_and_the_saved_retry_budget() {
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

    let model = Arc::new(ScriptedCompactionModel::new([
        tool_shaped_summary(),
        response(valid_summary(), StopReason::Stop, usage(6, 3)),
        response("done", StopReason::Stop, usage(7, 4)),
    ]));
    let profile = compacting_profile(model.clone());
    run_prompt(&harness, session_id, &profile, "three").await;

    assert_eq!(model.requests().len(), 3);
    let operation = &harness
        .inspect(session_id)
        .await
        .expect("inspect session")
        .operations[2];
    assert_eq!(operation.status, OperationStatus::Completed);
    assert_eq!(operation.model_usage.attempts, 3);
    assert_eq!(operation.model_usage.known, Some(usage(18, 9)));
}

#[tokio::test]
async fn an_invalid_summary_exhausts_only_its_checkpoint_retry_budget() {
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

    let model = Arc::new(ScriptedCompactionModel::new([response(
        "not a checkpoint",
        StopReason::Length,
        usage(5, 2),
    )]));
    let profile = compacting_profile_with_attempts(model.clone(), 1);
    let admission = harness
        .admit_standalone(
            session_id,
            OperationRequest::new(RequestId::new(), vec![ContentBlock::text("three")]),
        )
        .await
        .expect("admit operation");

    assert!(matches!(
        harness
            .run_next(session_id, &profile)
            .await
            .expect("settle failed operation"),
        RunNext::Finished {
            operation_id,
            outcome: OperationOutcome::Failed { .. },
        } if operation_id == admission.operation_id
    ));
    let snapshot = harness.inspect(session_id).await.expect("inspect session");
    assert_eq!(snapshot.operations[2].model_usage.known, Some(usage(5, 2)));
    assert_eq!(model.requests().len(), 1);
}

fn compacting_profile_with_attempts(model: Arc<dyn Model>, attempts: u32) -> RuntimeProfile {
    use renoa_harness::{CompactionPolicy, ContextSizer};
    use std::num::NonZeroU64;

    struct OversizedHistory;
    impl ContextSizer for OversizedHistory {
        fn estimate_input_tokens(&self, request: &ModelRequest) -> u64 {
            if request.system_prompt != "Be precise." {
                40
            } else if request.messages.len() >= 5 {
                90
            } else {
                10
            }
        }
    }

    RuntimeProfile::new(
        "compact-v1",
        model,
        "Be precise.",
        NonZeroU32::new(1).expect("non-zero model attempt limit"),
    )
    .with_compaction(
        CompactionPolicy::new(
            NonZeroU64::new(100).expect("non-zero context window"),
            20,
            NonZeroU64::new(50).expect("non-zero target"),
            NonZeroU64::new(40).expect("non-zero summary limit"),
            NonZeroU32::new(attempts).expect("non-zero compaction attempts"),
        )
        .expect("valid compaction policy"),
        Arc::new(OversizedHistory),
    )
}

struct ScriptedCompactionModel {
    responses: Mutex<VecDeque<ModelResponse>>,
    requests: Mutex<Vec<ModelRequest>>,
}

impl ScriptedCompactionModel {
    fn new(responses: impl IntoIterator<Item = ModelResponse>) -> Self {
        Self {
            responses: Mutex::new(responses.into_iter().collect()),
            requests: Mutex::new(Vec::new()),
        }
    }

    fn requests(&self) -> Vec<ModelRequest> {
        self.requests.lock().expect("request lock").clone()
    }
}

impl Model for ScriptedCompactionModel {
    fn stream(
        &self,
        request: ModelRequest,
        _cancellation: CancellationToken,
    ) -> ModelEventStream<'_> {
        self.requests.lock().expect("request lock").push(request);
        let response = self
            .responses
            .lock()
            .expect("response lock")
            .pop_front()
            .expect("scripted response");
        stream::once(async move { Ok(ModelEvent::Completed { response }) }).boxed()
    }
}

fn tool_shaped_summary() -> ModelResponse {
    ModelResponse {
        content: vec![AssistantContent::tool_call(ToolCall {
            id: "bad-call".to_owned(),
            name: "bash".to_owned(),
            arguments: serde_json::json!({"command": "false"}),
            thought_signature: None,
            namespace: None,
        })],
        stop_reason: StopReason::ToolUse,
        usage: Some(usage(5, 2)),
        metadata: AssistantMetadata::default(),
    }
}
