use std::{
    collections::VecDeque,
    num::NonZeroU32,
    sync::{Arc, Mutex},
};

use futures_util::{StreamExt, stream};
use renoa_agent::{
    AssistantContent, AssistantMetadata, BoxFuture, ContentBlock, Model, ModelError, ModelEvent,
    ModelEventStream, ModelRequest, ModelResponse, StopReason, Tool, ToolCall, ToolError,
    ToolOutput, ToolSpec, ToolUpdates,
};
use renoa_agent_loop::{
    AgentCommand, AgentLoopConfig, AgentToolBinding, ContextBinding, ModelBinding, build_runtime,
};
use renoa_kernel::{
    AgentId, Command, CommandId, DriveResult, EffectRecovery, EffectStatus, EventCursor, Kernel,
    OperationOutcome, OperationStatus, SessionId, SessionSnapshot,
};
use tempfile::tempdir;
use tokio_util::sync::CancellationToken;

#[tokio::test]
async fn interrupted_model_replays_the_exact_persisted_request() {
    let directory = tempdir().expect("temporary directory");
    let database = directory.path().join("kernel.sqlite3");
    let kernel = Arc::new(Kernel::open(&database).expect("open kernel"));
    let session_id = create_session(&kernel);
    submit_text(&kernel, session_id, "Remember this exact request.");
    let runtime = test_runtime(Arc::new(PanickingModel), Vec::new());
    let driver = Arc::clone(&kernel);
    let task = tokio::spawn(async move { driver.drive(session_id, &runtime).await });
    assert!(task.await.expect_err("model panic").is_panic());

    let interrupted = kernel
        .inspect(session_id)
        .expect("inspect interrupted model");
    let original_effect = interrupted.operations[0].effects[0].clone();
    assert_eq!(original_effect.status, EffectStatus::DispatchStarted);
    assert_eq!(original_effect.dispatch_count, 1);
    drop(kernel);

    let kernel = Kernel::open(&database).expect("reopen kernel");
    let requests = Arc::new(Mutex::new(Vec::new()));
    let resumed = test_runtime(
        Arc::new(RecordingModel::new(
            [text_response("Recovered.")],
            Arc::clone(&requests),
        )),
        Vec::new(),
    );
    assert!(matches!(
        kernel
            .drive(session_id, &resumed)
            .await
            .expect("recover model effect"),
        DriveResult::Finished { .. }
    ));

    let requests = requests.lock().expect("request lock");
    assert_eq!(requests.len(), 1);
    assert_eq!(
        requests[0].messages,
        vec![renoa_agent::Message::user_text(
            "Remember this exact request."
        )]
    );
    drop(requests);
    let recovered = kernel.inspect(session_id).expect("inspect recovered model");
    let replayed_effect = &recovered.operations[0].effects[0];
    assert_eq!(replayed_effect.effect_id, original_effect.effect_id);
    assert_eq!(replayed_effect.request, original_effect.request);
    assert_eq!(replayed_effect.dispatch_count, 2);
    assert_eq!(replayed_effect.status, EffectStatus::Settled);
}

#[tokio::test]
async fn interrupted_never_replay_tool_becomes_unknown_without_invocation() {
    let directory = tempdir().expect("temporary directory");
    let database = directory.path().join("kernel.sqlite3");
    let kernel = Arc::new(Kernel::open(&database).expect("open kernel"));
    let session_id = create_session(&kernel);
    submit_text(&kernel, session_id, "Perform the unsafe action.");
    let runtime = test_runtime(
        Arc::new(RecordingModel::new(
            [tool_response(tool_call("unsafe-1", "unsafe_action"))],
            Arc::new(Mutex::new(Vec::new())),
        )),
        vec![AgentToolBinding::new(
            "unsafe-action-v1",
            Arc::new(PanickingTool),
            EffectRecovery::NeverReplay,
        )],
    );
    let driver = Arc::clone(&kernel);
    let task = tokio::spawn(async move { driver.drive(session_id, &runtime).await });
    assert!(task.await.expect_err("tool panic").is_panic());
    let interrupted = kernel
        .inspect(session_id)
        .expect("inspect interrupted tool");
    assert_eq!(interrupted.operations[0].effects.len(), 2);
    assert_eq!(
        interrupted.operations[0].effects[1].status,
        EffectStatus::DispatchStarted
    );
    drop(kernel);

    let calls = Arc::new(Mutex::new(Vec::new()));
    let kernel = Kernel::open(&database).expect("reopen kernel");
    let resumed = test_runtime(
        Arc::new(NeverCalledModel),
        vec![AgentToolBinding::new(
            "unsafe-action-v1",
            Arc::new(RecordingTool {
                calls: Arc::clone(&calls),
            }),
            EffectRecovery::NeverReplay,
        )],
    );
    assert!(matches!(
        kernel
            .drive(session_id, &resumed)
            .await
            .expect("recover unsafe tool"),
        DriveResult::Blocked { .. }
    ));
    assert!(calls.lock().expect("tool calls lock").is_empty());
    let blocked = kernel.inspect(session_id).expect("inspect blocked tool");
    assert_eq!(
        blocked.operations[0].status,
        OperationStatus::OutcomeUnknown
    );
    assert_eq!(
        blocked.operations[0].effects[1].status,
        EffectStatus::OutcomeUnknown
    );
}

#[tokio::test]
async fn uncertain_model_failure_blocks_the_operation_without_settling_it() {
    let (result, blocked) = drive_one_model(Arc::new(UncertainModel)).await;
    assert!(matches!(result, DriveResult::Blocked { .. }));
    assert_eq!(
        blocked.operations[0].status,
        OperationStatus::OutcomeUnknown
    );
    assert_eq!(
        blocked.operations[0].effects[0].status,
        EffectStatus::OutcomeUnknown
    );
    assert_eq!(blocked.operations[0].effects[0].outcome, None);
}

#[tokio::test]
async fn incomplete_model_stream_blocks_the_operation_without_settling_it() {
    let (result, blocked) = drive_one_model(Arc::new(IncompleteModel)).await;
    assert!(matches!(result, DriveResult::Blocked { .. }));
    assert_eq!(
        blocked.operations[0].status,
        OperationStatus::OutcomeUnknown
    );
    assert_eq!(
        blocked.operations[0].effects[0].status,
        EffectStatus::OutcomeUnknown
    );
    assert_eq!(blocked.operations[0].effects[0].outcome, None);
}

#[tokio::test]
async fn known_pre_inference_rejection_remains_a_definite_failure() {
    let (result, failed) = drive_one_model(Arc::new(RejectedModel)).await;
    assert!(matches!(
        result,
        DriveResult::Finished {
            outcome: OperationOutcome::Failed { .. },
            ..
        }
    ));
    assert_eq!(failed.operations[0].status, OperationStatus::Failed);
    assert_eq!(
        failed.operations[0].effects[0].status,
        EffectStatus::Settled
    );
    assert!(failed.operations[0].effects[0].outcome.is_some());
}

#[tokio::test]
async fn uncertain_live_tool_outcome_blocks_without_recording_a_false_result() {
    let directory = tempdir().expect("temporary directory");
    let kernel = Kernel::open(directory.path().join("kernel.sqlite3")).expect("open kernel");
    let session_id = create_session(&kernel);
    submit_text(&kernel, session_id, "Perform the external action once.");
    let tool_calls = Arc::new(Mutex::new(Vec::new()));
    let model_requests = Arc::new(Mutex::new(Vec::new()));
    let runtime = test_runtime(
        Arc::new(RecordingModel::new(
            [tool_response(tool_call("unsafe-live-1", "unsafe_action"))],
            Arc::clone(&model_requests),
        )),
        vec![AgentToolBinding::new(
            "unsafe-action-v1",
            Arc::new(UncertainTool {
                calls: Arc::clone(&tool_calls),
            }),
            EffectRecovery::NeverReplay,
        )],
    );

    assert!(matches!(
        kernel
            .drive(session_id, &runtime)
            .await
            .expect("drive uncertain tool"),
        DriveResult::Blocked { .. }
    ));
    assert_eq!(tool_calls.lock().expect("tool calls lock").len(), 1);
    assert_eq!(model_requests.lock().expect("model requests lock").len(), 1);

    let blocked = kernel.inspect(session_id).expect("inspect blocked tool");
    assert_eq!(
        blocked.operations[0].status,
        OperationStatus::OutcomeUnknown
    );
    assert_eq!(blocked.operations[0].effects.len(), 2);
    assert_eq!(
        blocked.operations[0].effects[1].status,
        EffectStatus::OutcomeUnknown
    );
    assert_eq!(blocked.operations[0].effects[1].outcome, None);

    let history = kernel
        .events_after(session_id, EventCursor::START)
        .expect("read blocked history");
    assert_eq!(history.events.len(), 2);
    let messages = history
        .events
        .into_iter()
        .map(|event| {
            serde_json::from_value::<renoa_agent::Message>(event.payload)
                .expect("decode message event")
        })
        .collect::<Vec<_>>();
    assert!(matches!(messages[0], renoa_agent::Message::User { .. }));
    assert!(matches!(
        messages[1],
        renoa_agent::Message::Assistant { .. }
    ));
}

async fn drive_one_model(model: Arc<dyn Model>) -> (DriveResult, SessionSnapshot) {
    let directory = tempdir().expect("temporary directory");
    let kernel = Kernel::open(directory.path().join("kernel.sqlite3")).expect("open kernel");
    let session_id = create_session(&kernel);
    submit_text(&kernel, session_id, "Classify this model result honestly.");
    let runtime = test_runtime(model, Vec::new());
    let result = kernel
        .drive(session_id, &runtime)
        .await
        .expect("drive model result");
    let snapshot = kernel.inspect(session_id).expect("inspect model result");
    (result, snapshot)
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

fn submit_text(kernel: &Kernel, session_id: SessionId, text: &str) {
    let content = serde_json::to_value(AgentCommand::text(text)).expect("serialize command");
    kernel
        .submit(session_id, Command::new(CommandId::new(), content))
        .expect("submit command");
}

fn test_runtime(model: Arc<dyn Model>, tools: Vec<AgentToolBinding>) -> renoa_kernel::Runtime {
    build_runtime(
        AgentLoopConfig::new(
            "Recovery test.",
            NonZeroU32::new(4).expect("non-zero model limit"),
            NonZeroU32::new(4).expect("non-zero tool limit"),
        ),
        ContextBinding::full_history(),
        ModelBinding::new("recovery-model-v1", model, EffectRecovery::SafeToReplay),
        tools,
    )
    .expect("build runtime")
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
        panic!("model must not run while a tool outcome is unknown")
    }
}

struct UncertainModel;

impl Model for UncertainModel {
    fn stream(
        &self,
        _request: ModelRequest,
        _cancellation: CancellationToken,
    ) -> ModelEventStream<'_> {
        stream::once(async { Err(ModelError::new("provider connection was lost")) }).boxed()
    }
}

struct IncompleteModel;

impl Model for IncompleteModel {
    fn stream(
        &self,
        _request: ModelRequest,
        _cancellation: CancellationToken,
    ) -> ModelEventStream<'_> {
        stream::empty().boxed()
    }
}

struct RejectedModel;

impl Model for RejectedModel {
    fn stream(
        &self,
        _request: ModelRequest,
        _cancellation: CancellationToken,
    ) -> ModelEventStream<'_> {
        stream::once(async {
            Err(ModelError::context_window_exceeded(
                "model request exceeds its context window",
            ))
        })
        .boxed()
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

struct PanickingTool;

impl Tool for PanickingTool {
    fn spec(&self) -> &ToolSpec {
        unsafe_tool_spec()
    }

    fn execute(
        &self,
        _call: ToolCall,
        _cancellation: CancellationToken,
        _updates: ToolUpdates,
    ) -> BoxFuture<'_, Result<ToolOutput, ToolError>> {
        Box::pin(async { panic!("injected tool process loss") })
    }
}

struct RecordingTool {
    calls: Arc<Mutex<Vec<ToolCall>>>,
}

struct UncertainTool {
    calls: Arc<Mutex<Vec<ToolCall>>>,
}

impl Tool for UncertainTool {
    fn spec(&self) -> &ToolSpec {
        unsafe_tool_spec()
    }

    fn execute(
        &self,
        call: ToolCall,
        _cancellation: CancellationToken,
        _updates: ToolUpdates,
    ) -> BoxFuture<'_, Result<ToolOutput, ToolError>> {
        self.calls.lock().expect("tool calls lock").push(call);
        Box::pin(std::future::ready(Err(ToolError::outcome_unknown(
            "connection closed after dispatch",
        ))))
    }
}

impl Tool for RecordingTool {
    fn spec(&self) -> &ToolSpec {
        unsafe_tool_spec()
    }

    fn execute(
        &self,
        call: ToolCall,
        _cancellation: CancellationToken,
        _updates: ToolUpdates,
    ) -> BoxFuture<'_, Result<ToolOutput, ToolError>> {
        self.calls.lock().expect("tool calls lock").push(call);
        Box::pin(std::future::ready(Ok(ToolOutput {
            content: vec![ContentBlock::text("done")],
            details: None,
        })))
    }
}

fn unsafe_tool_spec() -> &'static ToolSpec {
    static SPEC: std::sync::OnceLock<ToolSpec> = std::sync::OnceLock::new();
    SPEC.get_or_init(|| ToolSpec {
        name: "unsafe_action".to_owned(),
        description: "Perform one unsafe test action.".to_owned(),
        input_schema: serde_json::json!({"type": "object"}),
    })
}

fn tool_call(id: &str, name: &str) -> ToolCall {
    ToolCall {
        id: id.to_owned(),
        name: name.to_owned(),
        arguments: serde_json::json!({}),
        thought_signature: None,
        namespace: None,
    }
}

fn tool_response(call: ToolCall) -> ModelResponse {
    ModelResponse {
        content: vec![AssistantContent::tool_call(call)],
        stop_reason: StopReason::ToolUse,
        usage: None,
        metadata: AssistantMetadata::default(),
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
