use std::{
    num::{NonZeroU32, NonZeroU64},
    sync::{Arc, Mutex},
};

use futures_util::{StreamExt, stream};
use renoa_agent::{
    AssistantContent, AssistantMetadata, ContentBlock, Model, ModelEvent, ModelEventStream,
    ModelRequest, ModelResponse, StopReason, TokenUsage,
};
use renoa_harness::{
    CompactionPolicy, CompactionPolicyError, ContextSizer, Harness, OperationRequest, RequestId,
    RuntimeProfile, SessionId,
};
use tempfile::tempdir;
use tokio::{sync::Notify, time::timeout};
use tokio_util::sync::CancellationToken;

#[path = "compaction/failures.rs"]
mod failures;
#[path = "compaction/provider_overflow.rs"]
mod provider_overflow;
#[path = "compaction/repeated.rs"]
mod repeated;
mod tool_support;
#[path = "compaction/tool_turn.rs"]
mod tool_turn;

#[test]
fn compaction_policy_rejects_invalid_budget_ordering() {
    let error = CompactionPolicy::new(
        NonZeroU64::new(100).expect("non-zero context window"),
        100,
        NonZeroU64::new(50).expect("non-zero target"),
        NonZeroU64::new(40).expect("non-zero summary limit"),
        NonZeroU32::new(2).expect("non-zero attempt limit"),
    )
    .expect_err("reserved tokens consume the context window");

    assert_eq!(
        error,
        CompactionPolicyError::ReservedTokensExhaustWindow {
            context_window_tokens: 100,
            reserved_tokens: 100,
        }
    );

    let error = CompactionPolicy::new(
        NonZeroU64::new(100).expect("non-zero context window"),
        20,
        NonZeroU64::new(80).expect("non-zero target"),
        NonZeroU64::new(40).expect("non-zero summary limit"),
        NonZeroU32::new(2).expect("non-zero attempt limit"),
    )
    .expect_err("target must leave dispatch headroom");
    assert_eq!(
        error,
        CompactionPolicyError::TargetNotBelowDispatchLimit {
            target_input_tokens: 80,
            dispatch_limit_tokens: 80,
        }
    );

    let error = CompactionPolicy::new(
        NonZeroU64::new(100).expect("non-zero context window"),
        20,
        NonZeroU64::new(50).expect("non-zero target"),
        NonZeroU64::new(50).expect("non-zero summary limit"),
        NonZeroU32::new(2).expect("non-zero attempt limit"),
    )
    .expect_err("summary budget must leave room for recent context");
    assert_eq!(
        error,
        CompactionPolicyError::SummaryNotBelowTarget {
            max_summary_tokens: 50,
            target_input_tokens: 50,
        }
    );
}

#[tokio::test]
async fn a_request_below_the_dispatch_limit_uses_one_normal_model_call() {
    let directory = tempdir().expect("temporary directory");
    let model = Arc::new(RecordingModel::default());
    let profile = RuntimeProfile::new(
        "compact-v1",
        model.clone(),
        "Be precise.",
        NonZeroU32::new(1).expect("non-zero model attempt limit"),
    )
    .with_compaction(
        CompactionPolicy::new(
            NonZeroU64::new(100).expect("non-zero context window"),
            20,
            NonZeroU64::new(50).expect("non-zero target"),
            NonZeroU64::new(40).expect("non-zero summary limit"),
            NonZeroU32::new(2).expect("non-zero compaction attempt limit"),
        )
        .expect("valid compaction policy"),
        Arc::new(FixedSizer(10)),
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
            OperationRequest::new(RequestId::new(), vec![ContentBlock::text("hello")]),
        )
        .await
        .expect("admit operation");

    harness
        .run_next(session_id, &profile)
        .await
        .expect("run operation");

    assert_eq!(
        model.requests(),
        vec![ModelRequest {
            system_prompt: "Be precise.".to_owned(),
            messages: vec![renoa_agent::Message::user_text("hello")],
            tools: Vec::new(),
        }]
    );
}

#[tokio::test]
async fn an_oversized_history_becomes_a_checkpoint_before_normal_sampling() {
    let directory = tempdir().expect("temporary directory");
    let model = Arc::new(CheckpointAwareModel::default());
    let profile = RuntimeProfile::new(
        "compact-v1",
        model.clone(),
        "Be precise.",
        NonZeroU32::new(1).expect("non-zero model attempt limit"),
    )
    .with_compaction(
        CompactionPolicy::new(
            NonZeroU64::new(100).expect("non-zero context window"),
            20,
            NonZeroU64::new(50).expect("non-zero target"),
            NonZeroU64::new(40).expect("non-zero summary limit"),
            NonZeroU32::new(2).expect("non-zero compaction attempt limit"),
        )
        .expect("valid compaction policy"),
        Arc::new(ShapeSizer),
    );
    let harness = Harness::open(directory.path().join("harness.sqlite3")).expect("open harness");
    let session_id = SessionId::new();
    harness
        .create_standalone_session(session_id)
        .await
        .expect("create session");

    run_prompt(&harness, session_id, &profile, "one").await;
    run_prompt(&harness, session_id, &profile, "two").await;
    run_prompt(&harness, session_id, &profile, "three").await;

    let requests = model.requests();
    assert_eq!(requests.len(), 4);
    assert_eq!(requests[2].tools, Vec::new());
    assert_ne!(requests[2].system_prompt, "Be precise.");
    assert_eq!(
        requests[3].messages.last(),
        Some(&renoa_agent::Message::user_text("three"))
    );
    let final_context = serde_json::to_string(&requests[3].messages).expect("encode context");
    assert!(final_context.contains("CONTEXT CHECKPOINT"));
    assert!(!final_context.contains("\"text\":\"one\""));
    assert!(final_context.contains("\"text\":\"two\""));

    let snapshot = harness.inspect(session_id).await.expect("inspect session");
    assert_eq!(
        snapshot.messages.len(),
        6,
        "compaction must not rewrite the durable transcript"
    );
    assert_eq!(snapshot.operations[2].model_usage.attempts, 2);
    assert_eq!(snapshot.operations[2].model_usage.attempts_without_usage, 2);
}

#[tokio::test]
async fn process_loss_retries_the_exact_saved_compaction_request_with_honest_uncertainty() {
    let directory = tempdir().expect("temporary directory");
    let database = directory.path().join("harness.sqlite3");
    let started = Arc::new(Notify::new());
    let first_model = Arc::new(PendingCompactionModel::new(Arc::clone(&started)));
    let first_profile = Arc::new(compacting_profile(first_model.clone()));
    let harness = Arc::new(Harness::open(&database).expect("open harness"));
    let session_id = SessionId::new();
    harness
        .create_standalone_session(session_id)
        .await
        .expect("create session");
    run_prompt(&harness, session_id, &first_profile, "one").await;
    run_prompt(&harness, session_id, &first_profile, "two").await;
    harness
        .admit_standalone(
            session_id,
            OperationRequest::new(RequestId::new(), vec![ContentBlock::text("three")]),
        )
        .await
        .expect("admit third operation");
    let driver = Arc::clone(&harness);
    let profile = Arc::clone(&first_profile);
    let run = tokio::spawn(async move { driver.run_next(session_id, &profile).await });
    timeout(std::time::Duration::from_secs(2), started.notified())
        .await
        .expect("compaction starts");
    run.abort();
    run.await.expect_err("driver is interrupted");
    let original_request = first_model
        .compaction_request()
        .expect("pending request was recorded");
    drop(first_profile);
    drop(harness);

    let recovered_model = Arc::new(CheckpointAwareModel::default());
    let recovered_profile = compacting_profile(recovered_model.clone());
    let harness = Harness::open(&database).expect("reopen harness");
    harness
        .run_next(session_id, &recovered_profile)
        .await
        .expect("recover operation");

    assert_eq!(recovered_model.requests()[0], original_request);
    let snapshot = harness.inspect(session_id).await.expect("inspect session");
    assert_eq!(snapshot.operations[2].model_usage.attempts, 3);
    assert_eq!(
        snapshot.operations[2].model_usage.outcome_unknown_attempts,
        1
    );
}

pub(crate) fn compacting_profile(model: Arc<dyn Model>) -> RuntimeProfile {
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
            NonZeroU32::new(2).expect("non-zero compaction attempt limit"),
        )
        .expect("valid compaction policy"),
        Arc::new(ShapeSizer),
    )
}

pub(crate) async fn run_prompt(
    harness: &Harness,
    session_id: SessionId,
    profile: &RuntimeProfile,
    prompt: &str,
) {
    harness
        .admit_standalone(
            session_id,
            OperationRequest::new(RequestId::new(), vec![ContentBlock::text(prompt)]),
        )
        .await
        .expect("admit operation");
    harness
        .run_next(session_id, profile)
        .await
        .expect("run operation");
}

struct FixedSizer(u64);

impl ContextSizer for FixedSizer {
    fn estimate_input_tokens(&self, _request: &ModelRequest) -> u64 {
        self.0
    }
}

struct ShapeSizer;

impl ContextSizer for ShapeSizer {
    fn estimate_input_tokens(&self, request: &ModelRequest) -> u64 {
        if request.system_prompt != "Be precise." {
            40
        } else if request.messages.iter().any(|message| {
            serde_json::to_string(message)
                .expect("encode message")
                .contains("CONTEXT CHECKPOINT")
        }) {
            30
        } else if request.messages.len() >= 5 {
            90
        } else {
            10
        }
    }
}

#[derive(Default)]
struct RecordingModel {
    requests: Mutex<Vec<ModelRequest>>,
}

impl RecordingModel {
    fn requests(&self) -> Vec<ModelRequest> {
        self.requests.lock().expect("request lock").clone()
    }
}

impl Model for RecordingModel {
    fn stream(
        &self,
        request: ModelRequest,
        _cancellation: CancellationToken,
    ) -> ModelEventStream<'_> {
        self.requests.lock().expect("request lock").push(request);
        stream::once(async {
            Ok(ModelEvent::Completed {
                response: ModelResponse {
                    content: vec![AssistantContent::text("done")],
                    stop_reason: StopReason::Stop,
                    usage: None,
                    metadata: AssistantMetadata::default(),
                },
            })
        })
        .boxed()
    }
}

#[derive(Default)]
pub(crate) struct CheckpointAwareModel {
    requests: Mutex<Vec<ModelRequest>>,
}

impl CheckpointAwareModel {
    pub(crate) fn requests(&self) -> Vec<ModelRequest> {
        self.requests.lock().expect("request lock").clone()
    }
}

impl Model for CheckpointAwareModel {
    fn stream(
        &self,
        request: ModelRequest,
        _cancellation: CancellationToken,
    ) -> ModelEventStream<'_> {
        let is_compaction = request.system_prompt != "Be precise.";
        self.requests.lock().expect("request lock").push(request);
        let output = if is_compaction {
            valid_summary()
        } else {
            "done"
        };
        stream::once(async move {
            Ok(ModelEvent::Completed {
                response: ModelResponse {
                    content: vec![AssistantContent::text(output)],
                    stop_reason: StopReason::Stop,
                    usage: None,
                    metadata: AssistantMetadata::default(),
                },
            })
        })
        .boxed()
    }
}

pub(crate) fn valid_summary() -> &'static str {
    "## Goal and user intent\nContinue the conversation.\n\
     ## Hard constraints and preferences\nBe precise.\n\
     ## Completed work\nCompleted earlier work.\n\
     ## Current state and blockers\nReady for the next request.\n\
     ## Decisions and rationale\nNo lasting decision.\n\
     ## Exact working facts\nNo paths, commands, or errors.\n\
     ## Next action and unresolved questions\nAnswer the active request."
}

pub(crate) fn response(text: &str, stop_reason: StopReason, usage: TokenUsage) -> ModelResponse {
    ModelResponse {
        content: vec![AssistantContent::text(text)],
        stop_reason,
        usage: Some(usage),
        metadata: AssistantMetadata::default(),
    }
}

pub(crate) fn usage(input: u64, output: u64) -> TokenUsage {
    TokenUsage {
        input,
        output,
        cache_read: 0,
        cache_write: 0,
    }
}

pub(crate) struct PendingCompactionModel {
    started: Arc<Notify>,
    compaction_request: Mutex<Option<ModelRequest>>,
}

impl PendingCompactionModel {
    pub(crate) fn new(started: Arc<Notify>) -> Self {
        Self {
            started,
            compaction_request: Mutex::new(None),
        }
    }

    pub(crate) fn compaction_request(&self) -> Option<ModelRequest> {
        self.compaction_request
            .lock()
            .expect("request lock")
            .clone()
    }
}

impl Model for PendingCompactionModel {
    fn stream(
        &self,
        request: ModelRequest,
        _cancellation: CancellationToken,
    ) -> ModelEventStream<'_> {
        if request.system_prompt != "Be precise." {
            *self.compaction_request.lock().expect("request lock") = Some(request);
            self.started.notify_one();
            return stream::pending().boxed();
        }
        stream::once(async {
            Ok(ModelEvent::Completed {
                response: ModelResponse {
                    content: vec![AssistantContent::text("done")],
                    stop_reason: StopReason::Stop,
                    usage: None,
                    metadata: AssistantMetadata::default(),
                },
            })
        })
        .boxed()
    }
}
