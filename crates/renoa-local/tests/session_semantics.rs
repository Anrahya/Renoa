use std::{
    collections::VecDeque,
    num::NonZeroU32,
    sync::{Arc, Mutex},
};

use futures_util::{StreamExt as _, stream};
use renoa_agent::{
    AssistantContent, AssistantMetadata, ContentBlock, Model, ModelError, ModelEvent,
    ModelEventStream, ModelRequest, ModelResponse, StopReason,
};
use renoa_agent_loop::{AgentLoopConfig, ContextBinding, ModelBinding, build_runtime};
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
