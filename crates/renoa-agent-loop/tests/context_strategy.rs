use std::{
    collections::VecDeque,
    num::NonZeroU32,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
};

use futures_util::{StreamExt, stream};
use renoa_agent::{
    AssistantContent, AssistantMetadata, Message, Model, ModelError, ModelEvent, ModelEventStream,
    ModelRequest, ModelResponse, StopReason,
};
use renoa_agent_loop::{
    AgentCommand, AgentLoopBuildError, AgentLoopConfig, ContextBinding, ContextInput,
    ContextStrategy, ContextStrategyError, ModelBinding, build_runtime,
};
use renoa_kernel::{
    AgentId, Command, CommandId, DriveResult, EffectRecovery, EffectStatus, EventCursor, Kernel,
    KernelError, OperationOutcome, SessionId,
};
use tempfile::tempdir;
use tokio_util::sync::CancellationToken;

#[tokio::test]
async fn strategy_changes_the_model_view_without_rewriting_durable_history() {
    let directory = tempdir().expect("temporary directory");
    let kernel = Kernel::open(directory.path().join("kernel.sqlite3")).expect("open kernel");
    let session_id = create_session(&kernel);
    let requests = Arc::new(Mutex::new(Vec::new()));
    let runtime = runtime(
        ContextBinding::new("latest-message-v1", Arc::new(LatestMessageStrategy)),
        Arc::new(RecordingModel::new(
            [
                text_response("First answer."),
                text_response("Second answer."),
            ],
            Arc::clone(&requests),
        )),
    );

    submit_and_drive(&kernel, session_id, &runtime, "First question.").await;
    submit_and_drive(&kernel, session_id, &runtime, "Second question.").await;

    let requests = requests.lock().expect("request lock");
    assert_eq!(requests.len(), 2);
    assert_eq!(
        requests[0].messages,
        vec![Message::user_text("First question.")]
    );
    assert_eq!(
        requests[1].messages,
        vec![Message::user_text("Second question.")]
    );
    drop(requests);

    let durable_messages = kernel
        .events_after(session_id, EventCursor::START)
        .expect("read durable history")
        .events
        .into_iter()
        .map(|event| serde_json::from_value(event.payload).expect("decode durable message"))
        .collect::<Vec<Message>>();
    assert_eq!(durable_messages.len(), 4);
    assert_eq!(durable_messages[0], Message::user_text("First question."));
    assert!(matches!(durable_messages[1], Message::Assistant { .. }));
    assert_eq!(durable_messages[2], Message::user_text("Second question."));
    assert!(matches!(durable_messages[3], Message::Assistant { .. }));
}

#[tokio::test]
async fn strategy_failure_is_retryable_and_precedes_model_dispatch() {
    let directory = tempdir().expect("temporary directory");
    let kernel = Kernel::open(directory.path().join("kernel.sqlite3")).expect("open kernel");
    let session_id = create_session(&kernel);
    let requests = Arc::new(Mutex::new(Vec::new()));
    let runtime = runtime(
        ContextBinding::new(
            "fail-once-v1",
            Arc::new(FailOnceStrategy {
                failed: AtomicBool::new(false),
            }),
        ),
        Arc::new(RecordingModel::new(
            [text_response("Recovered after projection.")],
            Arc::clone(&requests),
        )),
    );
    let operation_id = submit(&kernel, session_id, "Retry projection.");

    assert!(matches!(
        kernel.drive(session_id, &runtime).await,
        Err(KernelError::Loop(error))
            if error.message() == "context projection failed: injected projection failure"
    ));
    let failed = kernel
        .inspect(session_id)
        .expect("inspect failed projection");
    assert!(failed.operations[0].effects.is_empty());
    assert_eq!(
        kernel
            .events_after(session_id, EventCursor::START)
            .expect("read durable history")
            .events
            .len(),
        1
    );

    assert_eq!(
        kernel
            .drive(session_id, &runtime)
            .await
            .expect("retry projection"),
        DriveResult::Finished {
            operation_id,
            outcome: OperationOutcome::Completed,
        }
    );
    assert_eq!(requests.lock().expect("request lock").len(), 1);
}

#[tokio::test]
async fn recovery_reuses_the_persisted_projection_and_freezes_its_revision() {
    let directory = tempdir().expect("temporary directory");
    let database = directory.path().join("kernel.sqlite3");
    let kernel = Arc::new(Kernel::open(&database).expect("open kernel"));
    let session_id = create_session(&kernel);
    submit(&kernel, session_id, "Original durable message.");
    let initial_calls = Arc::new(AtomicUsize::new(0));
    let interrupted = runtime(
        fixed_context("projected message", Arc::clone(&initial_calls), "fixed-v1"),
        Arc::new(PanickingModel),
    );
    let driver = Arc::clone(&kernel);
    let task = tokio::spawn(async move { driver.drive(session_id, &interrupted).await });
    assert!(task.await.expect_err("model panic").is_panic());
    assert_eq!(initial_calls.load(Ordering::SeqCst), 1);

    let interrupted = kernel
        .inspect(session_id)
        .expect("inspect interrupted model");
    let original_effect = interrupted.operations[0].effects[0].clone();
    let original_request: ModelRequest =
        serde_json::from_value(original_effect.request.clone()).expect("decode model request");
    assert_eq!(
        original_request.messages,
        vec![Message::user_text("projected message")]
    );
    assert_eq!(original_effect.status, EffectStatus::DispatchStarted);
    drop(kernel);

    let kernel = Kernel::open(&database).expect("reopen kernel");
    let changed_calls = Arc::new(AtomicUsize::new(0));
    let changed = runtime(
        fixed_context("projected message", Arc::clone(&changed_calls), "fixed-v2"),
        Arc::new(NeverCalledModel),
    );
    assert!(matches!(
        kernel.drive(session_id, &changed).await,
        Err(KernelError::RuntimeMismatch)
    ));
    assert_eq!(changed_calls.load(Ordering::SeqCst), 0);

    let resumed_calls = Arc::new(AtomicUsize::new(0));
    let requests = Arc::new(Mutex::new(Vec::new()));
    let resumed = runtime(
        fixed_context("projected message", Arc::clone(&resumed_calls), "fixed-v1"),
        Arc::new(RecordingModel::new(
            [text_response("Recovered.")],
            Arc::clone(&requests),
        )),
    );
    assert!(matches!(
        kernel
            .drive(session_id, &resumed)
            .await
            .expect("resume model effect"),
        DriveResult::Finished {
            outcome: OperationOutcome::Completed,
            ..
        }
    ));
    assert_eq!(resumed_calls.load(Ordering::SeqCst), 1);
    assert_eq!(
        requests.lock().expect("request lock").as_slice(),
        [original_request]
    );

    let recovered = kernel.inspect(session_id).expect("inspect recovered model");
    let replayed_effect = &recovered.operations[0].effects[0];
    assert_eq!(replayed_effect.effect_id, original_effect.effect_id);
    assert_eq!(replayed_effect.request, original_effect.request);
    assert_eq!(replayed_effect.dispatch_count, 2);
    assert_eq!(replayed_effect.status, EffectStatus::Settled);
}

#[test]
fn empty_context_revision_is_rejected() {
    let result = build_runtime(
        config(),
        ContextBinding::new("", Arc::new(LatestMessageStrategy)),
        ModelBinding::new(
            "model-v1",
            Arc::new(NeverCalledModel),
            EffectRecovery::SafeToReplay,
        ),
        Vec::new(),
    );
    assert!(matches!(
        result,
        Err(AgentLoopBuildError::EmptyContextRevision)
    ));
}

struct LatestMessageStrategy;

impl ContextStrategy for LatestMessageStrategy {
    fn project(&self, input: ContextInput) -> Result<Vec<Message>, ContextStrategyError> {
        Ok(input
            .into_messages()
            .into_iter()
            .last()
            .into_iter()
            .collect())
    }
}

struct FailOnceStrategy {
    failed: AtomicBool,
}

impl ContextStrategy for FailOnceStrategy {
    fn project(&self, input: ContextInput) -> Result<Vec<Message>, ContextStrategyError> {
        if !self.failed.swap(true, Ordering::SeqCst) {
            return Err(ContextStrategyError::new("injected projection failure"));
        }
        Ok(input.into_messages())
    }
}

struct FixedContextStrategy {
    message: String,
    calls: Arc<AtomicUsize>,
}

impl ContextStrategy for FixedContextStrategy {
    fn project(&self, input: ContextInput) -> Result<Vec<Message>, ContextStrategyError> {
        assert!(!input.messages().is_empty());
        self.calls.fetch_add(1, Ordering::SeqCst);
        Ok(vec![Message::user_text(self.message.clone())])
    }
}

fn fixed_context(message: &str, calls: Arc<AtomicUsize>, revision: &str) -> ContextBinding {
    ContextBinding::new(
        revision,
        Arc::new(FixedContextStrategy {
            message: message.to_owned(),
            calls,
        }),
    )
}

struct PanickingModel;

impl Model for PanickingModel {
    fn stream(
        &self,
        _request: ModelRequest,
        _cancellation: CancellationToken,
    ) -> ModelEventStream<'_> {
        stream::once(async { panic!("injected model process loss") }).boxed()
    }
}

struct NeverCalledModel;

impl Model for NeverCalledModel {
    fn stream(
        &self,
        _request: ModelRequest,
        _cancellation: CancellationToken,
    ) -> ModelEventStream<'_> {
        panic!("model must not run")
    }
}

struct RecordingModel {
    responses: Mutex<VecDeque<ModelResponse>>,
    requests: Arc<Mutex<Vec<ModelRequest>>>,
}

impl RecordingModel {
    fn new(
        responses: impl IntoIterator<Item = ModelResponse>,
        requests: Arc<Mutex<Vec<ModelRequest>>>,
    ) -> Self {
        Self {
            responses: Mutex::new(responses.into_iter().collect()),
            requests,
        }
    }
}

impl Model for RecordingModel {
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
            .ok_or_else(|| ModelError::new("scripted model ran out of responses"));
        stream::once(async move { response.map(|response| ModelEvent::Completed { response }) })
            .boxed()
    }
}

fn config() -> AgentLoopConfig {
    AgentLoopConfig::new(
        "Test context projection.",
        NonZeroU32::new(4).expect("non-zero model limit"),
        NonZeroU32::new(2).expect("non-zero tool limit"),
    )
}

fn runtime(context: ContextBinding, model: Arc<dyn Model>) -> renoa_kernel::Runtime {
    build_runtime(
        config(),
        context,
        ModelBinding::new("model-v1", model, EffectRecovery::SafeToReplay),
        Vec::new(),
    )
    .expect("build runtime")
}

fn create_session(kernel: &Kernel) -> SessionId {
    let agent_id = AgentId::new();
    let session_id = SessionId::new();
    kernel.create_agent(agent_id).expect("create agent");
    kernel
        .create_session(session_id, agent_id)
        .expect("create session");
    session_id
}

fn submit(kernel: &Kernel, session_id: SessionId, text: &str) -> renoa_kernel::OperationId {
    kernel
        .submit(
            session_id,
            Command::new(
                CommandId::new(),
                serde_json::to_value(AgentCommand::text(text)).expect("serialize command"),
            ),
        )
        .expect("submit command")
        .operation_id
}

async fn submit_and_drive(
    kernel: &Kernel,
    session_id: SessionId,
    runtime: &renoa_kernel::Runtime,
    text: &str,
) {
    let operation_id = submit(kernel, session_id, text);
    assert_eq!(
        kernel
            .drive(session_id, runtime)
            .await
            .expect("drive operation"),
        DriveResult::Finished {
            operation_id,
            outcome: OperationOutcome::Completed,
        }
    );
}

fn text_response(text: &str) -> ModelResponse {
    ModelResponse {
        content: vec![AssistantContent::text(text)],
        stop_reason: StopReason::Stop,
        usage: None,
        metadata: AssistantMetadata::default(),
    }
}
