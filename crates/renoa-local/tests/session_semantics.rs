use std::{
    collections::VecDeque,
    num::{NonZeroU32, NonZeroU64},
    sync::{Arc, Mutex},
};

use futures_util::{StreamExt as _, stream};
use renoa_agent::{
    AssistantContent, AssistantMetadata, ContentBlock, Model, ModelError, ModelEvent,
    ModelEventStream, ModelRequest, ModelResponse, StopReason, TokenUsage,
};
use renoa_agent_loop::{
    AgentLoopConfig, CompactingContextStrategy, CompactionLimits, ContextBinding, ContextSizer,
    ModelBinding, build_runtime,
};
use renoa_kernel::{AgentId, CommandId, EffectRecovery, Kernel, SessionId};
use renoa_local::{LocalSession, LocalTurnOutcome};
use tempfile::tempdir;
use tokio_util::sync::CancellationToken;

#[tokio::test]
async fn a_pre_cancelled_turn_is_not_admitted_or_sent_to_the_model() {
    let directory = tempdir().expect("temporary directory");
    let database = directory.path().join("kernel.sqlite3");
    let agent_id = AgentId::new();
    let session_id = SessionId::new();
    let calls = Arc::new(Mutex::new(0_u32));
    let runtime = runtime(Arc::new(SequenceModel::new(
        [Ok(text_response("must not run"))],
        Arc::clone(&calls),
    )));
    let session = LocalSession::create(&database, agent_id, session_id).expect("create session");
    let cancellation = CancellationToken::new();
    cancellation.cancel();

    let outcome = session
        .execute_turn(
            CommandId::new(),
            vec![ContentBlock::text("Do not run")],
            &runtime,
            cancellation,
        )
        .await
        .expect("return pre-cancelled outcome");

    assert_eq!(outcome, LocalTurnOutcome::Cancelled);
    assert_eq!(*calls.lock().expect("model call counter"), 0);
    drop(session);
    assert!(
        Kernel::open(&database)
            .expect("reopen kernel")
            .inspect(session_id)
            .expect("inspect session")
            .operations
            .is_empty(),
        "pre-cancelled work must not leave a durable command"
    );
}

#[tokio::test]
async fn an_unknown_model_outcome_is_closed_honestly_and_the_session_remains_usable() {
    let directory = tempdir().expect("temporary directory");
    let calls = Arc::new(Mutex::new(0_u32));
    let runtime = runtime(Arc::new(SequenceModel::new(
        [
            Err(ModelError::new("provider reply was lost")),
            Ok(text_response("The next turn ran.")),
        ],
        Arc::clone(&calls),
    )));
    let session = LocalSession::create(
        directory.path().join("kernel.sqlite3"),
        AgentId::new(),
        SessionId::new(),
    )
    .expect("create session");

    let first = session
        .execute_turn(
            CommandId::new(),
            vec![ContentBlock::text("Lose this reply")],
            &runtime,
            CancellationToken::new(),
        )
        .await
        .expect("close unknown outcome");
    assert_eq!(
        first,
        LocalTurnOutcome::Failed {
            reason: "effect outcome is unknown; operation was abandoned without replay".to_owned(),
        }
    );

    let second = session
        .execute_turn(
            CommandId::new(),
            vec![ContentBlock::text("Continue safely")],
            &runtime,
            CancellationToken::new(),
        )
        .await
        .expect("run after unknown outcome");
    assert_eq!(
        second,
        LocalTurnOutcome::Completed {
            output: "The next turn ran.".to_owned(),
            stop_reason: StopReason::Stop,
        }
    );
    assert_eq!(*calls.lock().expect("model call counter"), 2);
}

#[tokio::test]
async fn explicit_compaction_is_projected_and_restored_as_the_latest_context_size() {
    let directory = tempdir().expect("temporary directory");
    let database = directory.path().join("kernel.sqlite3");
    let agent_id = AgentId::new();
    let session_id = SessionId::new();
    let calls = Arc::new(Mutex::new(0_u32));
    let runtime = compacting_runtime(Arc::new(SequenceModel::new(
        [
            Ok(text_response_with_usage(
                "First answer.",
                TokenUsage {
                    input: 11,
                    output: 7,
                    cache_read: 5,
                    cache_write: 3,
                },
            )),
            Ok(text_response(COMPACTION_SUMMARY)),
            Ok(text_response("Continued without provider usage.")),
        ],
        Arc::clone(&calls),
    )));
    let session = LocalSession::create(&database, agent_id, session_id).expect("create session");

    session
        .execute_turn(
            CommandId::new(),
            vec![ContentBlock::text("First question.")],
            &runtime,
            CancellationToken::new(),
        )
        .await
        .expect("execute first turn");
    assert_eq!(
        session.latest_context_tokens().expect("latest usage"),
        Some(26)
    );

    let command_id = CommandId::new();
    assert_eq!(
        session
            .execute_compaction(command_id, &runtime, CancellationToken::new())
            .await
            .expect("execute compaction"),
        LocalTurnOutcome::Compacted {
            estimated_input_tokens: 10,
        }
    );
    assert_eq!(
        session.latest_context_tokens().expect("compacted usage"),
        Some(10)
    );
    assert_eq!(
        session
            .replay_settled_compaction(command_id)
            .expect("replay compaction"),
        Some(LocalTurnOutcome::Compacted {
            estimated_input_tokens: 10,
        })
    );
    assert_eq!(*calls.lock().expect("model call counter"), 2);
    drop(session);

    let reopened = LocalSession::load(&database, session_id).expect("reopen session");
    assert_eq!(
        reopened.latest_context_tokens().expect("restored usage"),
        Some(10)
    );
    reopened
        .execute_turn(
            CommandId::new(),
            vec![ContentBlock::text("Continue.")],
            &runtime,
            CancellationToken::new(),
        )
        .await
        .expect("execute turn without usage");
    assert_eq!(
        reopened
            .latest_context_tokens()
            .expect("unknown latest usage"),
        None,
        "a newer unknown usage must not leave a stale donut value"
    );
    assert_eq!(*calls.lock().expect("model call counter"), 3);
}

fn runtime(model: Arc<dyn Model>) -> renoa_kernel::Runtime {
    build_runtime(
        AgentLoopConfig::new(
            "Test the local Host boundary.",
            NonZeroU32::new(2).expect("non-zero model limit"),
            NonZeroU32::new(1).expect("non-zero tool limit"),
        ),
        ContextBinding::full_history(),
        ModelBinding::new("test-model-v1", model, EffectRecovery::SafeToReplay),
        Vec::new(),
    )
    .expect("build runtime")
}

fn compacting_runtime(model: Arc<dyn Model>) -> renoa_kernel::Runtime {
    let limits = CompactionLimits::new(
        NonZeroU64::new(50).expect("non-zero context window"),
        10,
        NonZeroU64::new(30).expect("non-zero target"),
        NonZeroU64::new(10).expect("non-zero summary"),
    )
    .expect("valid limits");
    build_runtime(
        AgentLoopConfig::new(
            "Test the local Host boundary.",
            NonZeroU32::new(2).expect("non-zero model limit"),
            NonZeroU32::new(1).expect("non-zero tool limit"),
        ),
        ContextBinding::new(
            "manual-compaction-test-v1",
            Arc::new(CompactingContextStrategy::new(
                limits,
                NonZeroU32::new(2).expect("non-zero attempt limit"),
                Arc::new(TestSizer),
            )),
        ),
        ModelBinding::new("test-model-v1", model, EffectRecovery::SafeToReplay),
        Vec::new(),
    )
    .expect("build runtime")
}

struct TestSizer;

impl ContextSizer for TestSizer {
    fn estimate_input_tokens(&self, request: &ModelRequest) -> u64 {
        if request
            .system_prompt
            .starts_with("You create durable context checkpoints")
        {
            10
        } else {
            request
                .messages
                .iter()
                .map(|message| {
                    if serde_json::to_string(message)
                        .expect("encode message")
                        .contains("[CONTEXT CHECKPOINT]")
                    {
                        10
                    } else {
                        20
                    }
                })
                .sum()
        }
    }
}

struct SequenceModel {
    responses: Mutex<VecDeque<Result<ModelResponse, ModelError>>>,
    calls: Arc<Mutex<u32>>,
}

impl SequenceModel {
    fn new(
        responses: impl IntoIterator<Item = Result<ModelResponse, ModelError>>,
        calls: Arc<Mutex<u32>>,
    ) -> Self {
        Self {
            responses: Mutex::new(responses.into_iter().collect()),
            calls,
        }
    }
}

impl Model for SequenceModel {
    fn stream(
        &self,
        _request: ModelRequest,
        _cancellation: CancellationToken,
    ) -> ModelEventStream<'_> {
        *self.calls.lock().expect("model call counter") += 1;
        let response = self
            .responses
            .lock()
            .expect("model response queue")
            .pop_front()
            .expect("scripted model response");
        stream::once(async move { response.map(|response| ModelEvent::Completed { response }) })
            .boxed()
    }
}

fn text_response(text: &str) -> ModelResponse {
    ModelResponse {
        content: vec![AssistantContent::text(text)],
        stop_reason: StopReason::Stop,
        usage: None,
        metadata: AssistantMetadata::default(),
    }
}

fn text_response_with_usage(text: &str, usage: TokenUsage) -> ModelResponse {
    let mut response = text_response(text);
    response.usage = Some(usage);
    response
}

const COMPACTION_SUMMARY: &str = "## Goal and user intent\nContinue the task.\n\
## Hard constraints and preferences\nPreserve durable facts.\n\
## Completed work\nAnswered the first question.\n\
## Current state and blockers\nNo blocker.\n\
## Decisions and rationale\nUse a checkpoint.\n\
## Exact working facts\nThe first answer is durable.\n\
## Next action and unresolved questions\nAwait the next prompt.";
