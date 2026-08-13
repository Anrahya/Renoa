use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, Ordering},
};

use futures_util::{StreamExt, stream};
use renoa_agent::{
    AssistantContent, AssistantMetadata, BoxFuture, ContentBlock, ContextProjectionError,
    ContextProjector, Message, Model, ModelEvent, ModelEventStream, ModelRequest, ModelResponse,
    StopReason,
};
use renoa_harness::{
    CancellationId, Harness, OperationOutcome, OperationRequest, RequestId, RunNext,
    RuntimeProfile, SessionId,
};
use tempfile::tempdir;
use tokio::{sync::Notify, time::timeout};
use tokio_util::sync::CancellationToken;

#[tokio::test]
async fn projection_shapes_model_requests_without_rewriting_durable_history() {
    let directory = tempdir().expect("temporary directory");
    let model = Arc::new(RecordingModel::new(["first", "second"]));
    let projector = Arc::new(RecordingProjector::default());
    let profile = RuntimeProfile::new(
        "projected-v1",
        model.clone(),
        "Be precise.",
        std::num::NonZeroU32::new(1).expect("non-zero attempt limit"),
    )
    .with_context_projector(projector.clone());
    let harness = Harness::open(directory.path().join("harness.sqlite3")).expect("open harness");
    let session_id = SessionId::new();
    harness
        .create_standalone_session(session_id)
        .await
        .expect("create session");

    run_prompt(&harness, session_id, &profile, "one").await;
    run_prompt(&harness, session_id, &profile, "two").await;

    assert_eq!(
        projector.inputs(),
        vec![
            vec![Message::user_text("one")],
            vec![
                Message::user_text("one"),
                assistant("first"),
                Message::user_text("two"),
            ],
        ]
    );
    assert_eq!(
        model
            .requests()
            .into_iter()
            .map(|request| request.messages)
            .collect::<Vec<_>>(),
        vec![
            vec![Message::user_text("projection-1")],
            vec![Message::user_text("projection-2")],
        ]
    );
    assert_eq!(
        harness
            .inspect(session_id)
            .await
            .expect("inspect session")
            .messages,
        vec![
            Message::user_text("one"),
            assistant("first"),
            Message::user_text("two"),
            assistant("second"),
        ]
    );
}

#[tokio::test]
async fn recovery_reuses_the_persisted_projection_without_running_it_again() {
    let directory = tempdir().expect("temporary directory");
    let database = directory.path().join("harness.sqlite3");
    let started = Arc::new(Notify::new());
    let first_model = Arc::new(PendingModel::new(Arc::clone(&started)));
    let projector = Arc::new(RecordingProjector::default());
    let first_profile = Arc::new(
        RuntimeProfile::new(
            "projected-v1",
            first_model.clone(),
            "Be precise.",
            std::num::NonZeroU32::new(2).expect("non-zero attempt limit"),
        )
        .with_context_projector(projector.clone()),
    );
    let harness = Arc::new(Harness::open(&database).expect("open harness"));
    let session_id = SessionId::new();
    harness
        .create_standalone_session(session_id)
        .await
        .expect("create session");
    harness
        .admit_standalone(
            session_id,
            OperationRequest::new(RequestId::new(), vec![ContentBlock::text("one")]),
        )
        .await
        .expect("admit operation");
    let driver = Arc::clone(&harness);
    let profile = Arc::clone(&first_profile);
    let run = tokio::spawn(async move { driver.run_next(session_id, &profile).await });
    timeout(std::time::Duration::from_secs(2), started.notified())
        .await
        .expect("model request starts");
    run.abort();
    run.await.expect_err("driver is interrupted");
    drop(first_profile);
    drop(harness);

    let second_model = Arc::new(RecordingModel::new(["recovered"]));
    let second_profile = RuntimeProfile::new(
        "projected-v1",
        second_model.clone(),
        "Be precise.",
        std::num::NonZeroU32::new(2).expect("non-zero attempt limit"),
    )
    .with_context_projector(projector.clone());
    let harness = Harness::open(&database).expect("reopen harness");
    harness
        .run_next(session_id, &second_profile)
        .await
        .expect("recover operation");

    assert_eq!(projector.inputs(), vec![vec![Message::user_text("one")]]);
    assert_eq!(
        first_model.requests(),
        vec![ModelRequest {
            system_prompt: "Be precise.".to_owned(),
            messages: vec![Message::user_text("projection-1")],
            tools: Vec::new(),
        }]
    );
    assert_eq!(second_model.requests(), first_model.requests());
}

#[tokio::test]
async fn durable_cancellation_stops_projection_before_model_dispatch() {
    let directory = tempdir().expect("temporary directory");
    let started = Arc::new(Notify::new());
    let profile = Arc::new(
        RuntimeProfile::new(
            "projected-v1",
            Arc::new(NeverCalledModel),
            "Be precise.",
            std::num::NonZeroU32::new(1).expect("non-zero attempt limit"),
        )
        .with_context_projector(Arc::new(PendingProjector::new(Arc::clone(&started)))),
    );
    let harness =
        Arc::new(Harness::open(directory.path().join("harness.sqlite3")).expect("open harness"));
    let session_id = SessionId::new();
    harness
        .create_standalone_session(session_id)
        .await
        .expect("create session");
    let admission = harness
        .admit_standalone(
            session_id,
            OperationRequest::new(RequestId::new(), vec![ContentBlock::text("one")]),
        )
        .await
        .expect("admit operation");
    let driver = Arc::clone(&harness);
    let driver_profile = Arc::clone(&profile);
    let run = tokio::spawn(async move { driver.run_next(session_id, &driver_profile).await });
    timeout(std::time::Duration::from_secs(2), started.notified())
        .await
        .expect("projection starts");

    harness
        .request_standalone_cancellation(session_id, admission.operation_id, CancellationId::new())
        .await
        .expect("request cancellation");

    assert!(matches!(
        run.await
            .expect("join driver")
            .expect("settle cancellation"),
        RunNext::Finished {
            outcome: OperationOutcome::Cancelled { .. },
            ..
        }
    ));
}

#[tokio::test]
async fn projection_failure_leaves_the_operation_safe_to_retry() {
    let directory = tempdir().expect("temporary directory");
    let model = Arc::new(RecordingModel::new(["recovered"]));
    let profile = RuntimeProfile::new(
        "projected-v1",
        model.clone(),
        "Be precise.",
        std::num::NonZeroU32::new(1).expect("non-zero attempt limit"),
    )
    .with_context_projector(Arc::new(FailingOnceProjector::default()));
    let harness = Harness::open(directory.path().join("harness.sqlite3")).expect("open harness");
    let session_id = SessionId::new();
    harness
        .create_standalone_session(session_id)
        .await
        .expect("create session");
    harness
        .admit_standalone(
            session_id,
            OperationRequest::new(RequestId::new(), vec![ContentBlock::text("one")]),
        )
        .await
        .expect("admit operation");

    let error = harness
        .run_next(session_id, &profile)
        .await
        .expect_err("first projection fails");
    assert!(matches!(
        error,
        renoa_harness::HarnessError::ContextProjection(error)
            if error.to_string() == "temporary projection failure"
    ));
    assert!(model.requests().is_empty());

    harness
        .run_next(session_id, &profile)
        .await
        .expect("retry operation");
    assert_eq!(
        model.requests()[0].messages,
        vec![Message::user_text("projected")]
    );
}

async fn run_prompt(
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

#[derive(Default)]
struct RecordingProjector {
    inputs: Mutex<Vec<Vec<Message>>>,
}

impl RecordingProjector {
    fn inputs(&self) -> Vec<Vec<Message>> {
        self.inputs.lock().expect("projection lock").clone()
    }
}

impl ContextProjector for RecordingProjector {
    fn project(
        &self,
        messages: Vec<Message>,
        _cancellation: CancellationToken,
    ) -> BoxFuture<'_, Result<Vec<Message>, ContextProjectionError>> {
        let projection_number = {
            let mut inputs = self.inputs.lock().expect("projection lock");
            inputs.push(messages);
            inputs.len()
        };
        Box::pin(async move {
            Ok(vec![Message::user_text(format!(
                "projection-{projection_number}"
            ))])
        })
    }
}

struct PendingProjector {
    started: Arc<Notify>,
}

impl PendingProjector {
    fn new(started: Arc<Notify>) -> Self {
        Self { started }
    }
}

impl ContextProjector for PendingProjector {
    fn project(
        &self,
        _messages: Vec<Message>,
        _cancellation: CancellationToken,
    ) -> BoxFuture<'_, Result<Vec<Message>, ContextProjectionError>> {
        self.started.notify_one();
        Box::pin(std::future::pending())
    }
}

#[derive(Default)]
struct FailingOnceProjector {
    failed: AtomicBool,
}

impl ContextProjector for FailingOnceProjector {
    fn project(
        &self,
        _messages: Vec<Message>,
        _cancellation: CancellationToken,
    ) -> BoxFuture<'_, Result<Vec<Message>, ContextProjectionError>> {
        let fail = !self.failed.swap(true, Ordering::SeqCst);
        Box::pin(async move {
            if fail {
                Err(ContextProjectionError::new("temporary projection failure"))
            } else {
                Ok(vec![Message::user_text("projected")])
            }
        })
    }
}

struct RecordingModel {
    responses: Mutex<std::collections::VecDeque<String>>,
    requests: Mutex<Vec<ModelRequest>>,
}

struct PendingModel {
    started: Arc<Notify>,
    requests: Mutex<Vec<ModelRequest>>,
}

impl PendingModel {
    fn new(started: Arc<Notify>) -> Self {
        Self {
            started,
            requests: Mutex::new(Vec::new()),
        }
    }

    fn requests(&self) -> Vec<ModelRequest> {
        self.requests.lock().expect("request lock").clone()
    }
}

impl Model for PendingModel {
    fn stream(
        &self,
        request: ModelRequest,
        _cancellation: CancellationToken,
    ) -> ModelEventStream<'_> {
        self.requests.lock().expect("request lock").push(request);
        self.started.notify_one();
        stream::pending().boxed()
    }
}

struct NeverCalledModel;

impl Model for NeverCalledModel {
    fn stream(
        &self,
        _request: ModelRequest,
        _cancellation: CancellationToken,
    ) -> ModelEventStream<'_> {
        panic!("model must not be invoked while projection is pending")
    }
}

impl RecordingModel {
    fn new<const N: usize>(responses: [&str; N]) -> Self {
        Self {
            responses: Mutex::new(responses.into_iter().map(str::to_owned).collect()),
            requests: Mutex::new(Vec::new()),
        }
    }

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
        let response = self
            .responses
            .lock()
            .expect("response lock")
            .pop_front()
            .expect("scripted response");
        stream::once(async move {
            Ok(ModelEvent::Completed {
                response: ModelResponse {
                    content: vec![AssistantContent::text(response)],
                    stop_reason: StopReason::Stop,
                    usage: None,
                    metadata: AssistantMetadata::default(),
                },
            })
        })
        .boxed()
    }
}

fn assistant(text: &str) -> Message {
    Message::Assistant {
        content: vec![AssistantContent::text(text)],
        stop_reason: StopReason::Stop,
        usage: None,
        metadata: AssistantMetadata::default(),
    }
}
