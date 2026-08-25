use std::{
    collections::VecDeque,
    num::NonZeroU32,
    sync::{Arc, Mutex},
};

use futures_util::{StreamExt, stream};
use renoa_agent::{
    AssistantContent, AssistantMetadata, BoxFuture, ContentBlock, InferenceOutcome, Model,
    ModelError, ModelErrorKind, ModelEvent, ModelEventStream, ModelRequest, ModelResponse,
    StopReason, Tool, ToolCall, ToolError, ToolOutput, ToolSpec, ToolUpdates,
};
use renoa_agent_loop::{
    AgentCommand, AgentLoopConfig, AgentToolBinding, ContextBinding, ModelBinding, build_runtime,
};
use renoa_kernel::{
    AgentId, Command, CommandId, DriveResult, EffectRecovery, Kernel, SessionId, SessionSnapshot,
};
use tempfile::tempdir;
use tokio_util::sync::CancellationToken;

pub(crate) async fn drive_one_model(model: Arc<dyn Model>) -> (DriveResult, SessionSnapshot) {
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

pub(crate) fn create_session(kernel: &Kernel) -> SessionId {
    let agent_id = AgentId::new();
    let session_id = SessionId::new();
    kernel.create_agent(agent_id).expect("create agent");
    kernel
        .create_session(session_id, agent_id)
        .expect("create session");
    session_id
}

pub(crate) fn submit_text(kernel: &Kernel, session_id: SessionId, text: &str) {
    let content = serde_json::to_value(AgentCommand::text(text)).expect("serialize command");
    kernel
        .submit(session_id, Command::new(CommandId::new(), content))
        .expect("submit command");
}

pub(crate) fn test_runtime(
    model: Arc<dyn Model>,
    tools: Vec<AgentToolBinding>,
) -> renoa_kernel::Runtime {
    test_runtime_with_revision("recovery-model-v1", model, tools)
}

pub(crate) fn test_runtime_with_revision(
    revision: &str,
    model: Arc<dyn Model>,
    tools: Vec<AgentToolBinding>,
) -> renoa_kernel::Runtime {
    build_runtime(
        AgentLoopConfig::new(
            "Recovery test.",
            NonZeroU32::new(4).expect("non-zero model limit"),
            NonZeroU32::new(4).expect("non-zero tool limit"),
        ),
        ContextBinding::full_history(),
        ModelBinding::new(revision, model, EffectRecovery::SafeToReplay),
        tools,
    )
    .expect("build runtime")
}

pub(crate) struct PanickingModel;

impl Model for PanickingModel {
    fn stream(
        &self,
        _request: ModelRequest,
        _cancellation: CancellationToken,
    ) -> ModelEventStream<'_> {
        stream::once(async { panic!("injected model process loss") }).boxed()
    }
}

pub(crate) struct NeverCalledModel;

impl Model for NeverCalledModel {
    fn stream(
        &self,
        _request: ModelRequest,
        _cancellation: CancellationToken,
    ) -> ModelEventStream<'_> {
        panic!("model must not run while a tool outcome is unknown")
    }
}

pub(crate) struct UncertainModel;

impl Model for UncertainModel {
    fn stream(
        &self,
        _request: ModelRequest,
        _cancellation: CancellationToken,
    ) -> ModelEventStream<'_> {
        stream::once(async { Err(ModelError::new("provider connection was lost")) }).boxed()
    }
}

pub(crate) struct IncompleteModel;

impl Model for IncompleteModel {
    fn stream(
        &self,
        _request: ModelRequest,
        _cancellation: CancellationToken,
    ) -> ModelEventStream<'_> {
        stream::empty().boxed()
    }
}

pub(crate) struct RejectedModel;

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

pub(crate) struct NetworkRejectedModel;

impl Model for NetworkRejectedModel {
    fn stream(
        &self,
        _request: ModelRequest,
        _cancellation: CancellationToken,
    ) -> ModelEventStream<'_> {
        stream::once(async {
            Err(ModelError::classified(
                ModelErrorKind::Network,
                InferenceOutcome::KnownNotStarted,
                "xAI request failed after 3 attempts: connection refused before dispatch (ECONNREFUSED).",
                None,
            ))
        })
        .boxed()
    }
}

pub(crate) struct CompletesAfterCancelModel {
    pub(crate) invoked: Arc<tokio::sync::Notify>,
}

impl Model for CompletesAfterCancelModel {
    fn stream(
        &self,
        _request: ModelRequest,
        cancellation: CancellationToken,
    ) -> ModelEventStream<'_> {
        let invoked = Arc::clone(&self.invoked);
        stream::unfold(0_u8, move |step| {
            let cancellation = cancellation.clone();
            let invoked = Arc::clone(&invoked);
            async move {
                match step {
                    0 => {
                        invoked.notify_one();
                        Some((
                            Ok(ModelEvent::ProviderRequest {
                                payload: serde_json::json!({ "dispatched": true }),
                            }),
                            1,
                        ))
                    }
                    1 => {
                        cancellation.cancelled().await;
                        Some((
                            Ok(ModelEvent::Completed {
                                response: text_response("definite"),
                            }),
                            2,
                        ))
                    }
                    _ => None,
                }
            }
        })
        .boxed()
    }
}

pub(crate) struct PostDispatchResetModel;

impl Model for PostDispatchResetModel {
    fn stream(
        &self,
        _request: ModelRequest,
        _cancellation: CancellationToken,
    ) -> ModelEventStream<'_> {
        stream::iter([
            Ok(ModelEvent::ProviderRequest {
                payload: serde_json::json!({ "dispatched": true }),
            }),
            Err(ModelError::classified(
                ModelErrorKind::Network,
                InferenceOutcome::Unknown,
                "xAI request failed after 3 attempts: connection reset after the request was transmitted.",
                None,
            )),
        ])
        .boxed()
    }
}

pub(crate) struct RecordingModel {
    responses: Mutex<VecDeque<ModelResponse>>,
    requests: Arc<Mutex<Vec<ModelRequest>>>,
}

impl RecordingModel {
    pub(crate) fn new(
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

pub(crate) struct PanickingTool;

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

pub(crate) struct RecordingTool {
    pub(crate) calls: Arc<Mutex<Vec<ToolCall>>>,
}

pub(crate) struct UncertainTool {
    pub(crate) calls: Arc<Mutex<Vec<ToolCall>>>,
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

pub(crate) fn unsafe_tool_spec() -> &'static ToolSpec {
    static SPEC: std::sync::OnceLock<ToolSpec> = std::sync::OnceLock::new();
    SPEC.get_or_init(|| ToolSpec {
        name: "unsafe_action".to_owned(),
        description: "Perform one unsafe test action.".to_owned(),
        input_schema: serde_json::json!({"type": "object"}),
    })
}

pub(crate) fn tool_call(id: &str, name: &str) -> ToolCall {
    ToolCall {
        id: id.to_owned(),
        name: name.to_owned(),
        arguments: serde_json::json!({}),
        thought_signature: None,
        namespace: None,
    }
}

pub(crate) fn tool_response(call: ToolCall) -> ModelResponse {
    ModelResponse {
        content: vec![AssistantContent::tool_call(call)],
        stop_reason: StopReason::ToolUse,
        usage: None,
        metadata: AssistantMetadata::default(),
    }
}

pub(crate) fn text_response(text: &str) -> ModelResponse {
    ModelResponse {
        content: vec![AssistantContent::text(text)],
        stop_reason: StopReason::Stop,
        usage: None,
        metadata: AssistantMetadata::default(),
    }
}
