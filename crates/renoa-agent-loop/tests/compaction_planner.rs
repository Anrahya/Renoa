use std::{
    collections::VecDeque,
    num::{NonZeroU32, NonZeroU64},
    sync::{Arc, Mutex},
};

use futures_util::{StreamExt, stream};
use renoa_agent::{
    AssistantContent, AssistantMetadata, Message, Model, ModelError, ModelEvent, ModelEventStream,
    ModelRequest, ModelResponse, StopReason,
};
use renoa_agent_loop::{
    AgentCommand, AgentLoopConfig, CompactionLimits, CompactionPlan, CompactionPlanner,
    ContextBinding, ContextInput, ContextSizer, ContextStrategy, ContextStrategyError,
    ModelBinding, build_runtime,
};
use renoa_kernel::{
    AgentId, Command, CommandId, DriveResult, EffectRecovery, EventCursor, Kernel,
    OperationOutcome, SessionId,
};
use tempfile::tempdir;
use tokio_util::sync::CancellationToken;

#[tokio::test]
async fn planner_uses_durable_origins_through_the_real_kernel_loop() {
    let directory = tempdir().expect("temporary directory");
    let kernel = Kernel::open(directory.path().join("kernel.sqlite3")).expect("open kernel");
    let session_id = create_session(&kernel);
    let plans = Arc::new(Mutex::new(Vec::new()));
    let model_requests = Arc::new(Mutex::new(Vec::new()));
    let runtime = runtime(Arc::clone(&plans), Arc::clone(&model_requests));

    submit_and_drive(&kernel, session_id, &runtime, "First question.").await;
    submit_and_drive(&kernel, session_id, &runtime, "Second question.").await;

    let plans = plans.lock().expect("plan lock");
    assert_eq!(plans.len(), 4, "request creation and settlement must agree");
    assert!(plans[0].is_none());
    assert!(plans[1].is_none());
    assert_eq!(plans[2], plans[3]);
    let plan = plans[2]
        .as_ref()
        .expect("second operation has a safe prefix");
    assert_eq!(plan.covered_through_sequence(), 1);
    let summary = serde_json::to_string(plan.summary_request()).expect("encode summary request");
    assert!(summary.contains("First question."));
    assert!(summary.contains("First answer."));
    assert!(!summary.contains("Second question."));
    drop(plans);

    let model_requests = model_requests.lock().expect("model request lock");
    assert_eq!(model_requests.len(), 2);
    assert_eq!(model_requests[1].messages.len(), 3);
    assert_eq!(
        model_requests[1].messages[0],
        Message::user_text("First question.")
    );
    assert!(matches!(
        model_requests[1].messages[1],
        Message::Assistant { .. }
    ));
    assert_eq!(
        model_requests[1].messages[2],
        Message::user_text("Second question.")
    );
    drop(model_requests);

    let durable = kernel
        .events_after(session_id, EventCursor::START)
        .expect("read durable journal");
    assert_eq!(durable.events.len(), 4);
}

struct PlanningStrategy {
    planner: CompactionPlanner,
    plans: Arc<Mutex<Vec<Option<CompactionPlan>>>>,
}

impl ContextStrategy for PlanningStrategy {
    fn project(&self, input: ContextInput) -> Result<Vec<Message>, ContextStrategyError> {
        let plan = self
            .planner
            .plan(&input, None, "Test planning.", &[], &MessageCountSizer)
            .map_err(|error| ContextStrategyError::new(error.to_string()))?;
        self.plans.lock().expect("plan lock").push(plan);
        Ok(input.into_messages())
    }
}

struct MessageCountSizer;

impl ContextSizer for MessageCountSizer {
    fn estimate_input_tokens(&self, request: &ModelRequest) -> u64 {
        if request.system_prompt == "Test planning." {
            u64::try_from(request.messages.len()).expect("message count fits u64") * 10
        } else {
            10
        }
    }
}

struct RecordingModel {
    responses: Mutex<VecDeque<ModelResponse>>,
    requests: Arc<Mutex<Vec<ModelRequest>>>,
}

impl RecordingModel {
    fn new(requests: Arc<Mutex<Vec<ModelRequest>>>) -> Self {
        Self {
            responses: Mutex::new(
                [
                    text_response("First answer."),
                    text_response("Second answer."),
                ]
                .into_iter()
                .collect(),
            ),
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
        self.requests
            .lock()
            .expect("model request lock")
            .push(request);
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

fn runtime(
    plans: Arc<Mutex<Vec<Option<CompactionPlan>>>>,
    requests: Arc<Mutex<Vec<ModelRequest>>>,
) -> renoa_kernel::Runtime {
    let limits =
        CompactionLimits::new(nz(100), 20, nz(50), nz(40)).expect("valid compaction limits");
    build_runtime(
        AgentLoopConfig::new(
            "Test planning.",
            NonZeroU32::new(4).expect("non-zero model limit"),
            NonZeroU32::new(2).expect("non-zero tool limit"),
        ),
        ContextBinding::new(
            "planning-v1",
            Arc::new(PlanningStrategy {
                planner: CompactionPlanner::new(limits),
                plans,
            }),
        ),
        ModelBinding::new(
            "model-v1",
            Arc::new(RecordingModel::new(requests)),
            EffectRecovery::SafeToReplay,
        ),
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

async fn submit_and_drive(
    kernel: &Kernel,
    session_id: SessionId,
    runtime: &renoa_kernel::Runtime,
    text: &str,
) {
    let operation_id = kernel
        .submit(
            session_id,
            Command::new(
                CommandId::new(),
                serde_json::to_value(AgentCommand::text(text)).expect("serialize command"),
            ),
        )
        .expect("submit command")
        .operation_id;
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

fn nz(value: u64) -> NonZeroU64 {
    NonZeroU64::new(value).expect("test value is non-zero")
}
