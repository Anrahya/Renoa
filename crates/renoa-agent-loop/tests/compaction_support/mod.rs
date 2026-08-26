use std::{
    collections::VecDeque,
    num::{NonZeroU32, NonZeroU64},
    sync::{Arc, Mutex},
};

use futures_util::{StreamExt, stream};
use renoa_agent::{
    AssistantContent, AssistantMetadata, Model, ModelError, ModelEvent, ModelEventStream,
    ModelRequest, ModelResponse, StopReason, ToolCall,
};
use renoa_agent_loop::{
    AgentCommand, AgentLoopConfig, AgentToolBinding, CompactingContextStrategy, CompactionLimits,
    ContextBinding, ContextSizer, ContextStrategy, ModelBinding, build_runtime,
};
use renoa_kernel::{
    AgentId, Command, CommandId, DriveResult, EffectRecovery, Kernel, OperationOutcome, SessionId,
};
use tokio::sync::Notify;
use tokio_util::sync::CancellationToken;

pub(crate) const SUMMARY: &str = "## Goal and user intent\nKeep answering the user.\n\
## Hard constraints and preferences\nPreserve exact durable facts.\n\
## Completed work\nAnswered the first question.\n\
## Current state and blockers\nThe second question is active.\n\
## Decisions and rationale\nUse a durable checkpoint.\n\
## Exact working facts\nThe first answer was recorded.\n\
## Next action and unresolved questions\nAnswer the active question.";

pub(crate) fn compacting_runtime(
    model: Arc<dyn Model>,
    sizer: Arc<dyn ContextSizer>,
    max_attempts: NonZeroU32,
) -> renoa_kernel::Runtime {
    runtime_with_context(
        model,
        Arc::new(compacting_strategy(sizer, max_attempts)),
        "test-durable-compaction-v1",
    )
}

pub(crate) fn compacting_strategy(
    sizer: Arc<dyn ContextSizer>,
    max_attempts: NonZeroU32,
) -> CompactingContextStrategy {
    let limits =
        CompactionLimits::new(nz64(50), 10, nz64(30), nz64(10)).expect("valid compaction limits");
    CompactingContextStrategy::new(limits, max_attempts, sizer)
}

pub(crate) fn runtime_with_context(
    model: Arc<dyn Model>,
    context: Arc<dyn ContextStrategy>,
    context_revision: &str,
) -> renoa_kernel::Runtime {
    runtime_with_context_and_tools(model, context, context_revision, Vec::new())
}

pub(crate) fn runtime_with_context_and_tools(
    model: Arc<dyn Model>,
    context: Arc<dyn ContextStrategy>,
    context_revision: &str,
    tools: Vec<AgentToolBinding>,
) -> renoa_kernel::Runtime {
    build_runtime(
        AgentLoopConfig::new("Test durable compaction.", nz32(4), nz32(2)),
        ContextBinding::new(context_revision, context),
        ModelBinding::new("model-v1", model, EffectRecovery::SafeToReplay),
        tools,
    )
    .expect("build runtime")
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

pub(crate) fn submit(
    kernel: &Kernel,
    session_id: SessionId,
    text: &str,
) -> renoa_kernel::OperationId {
    submit_command(kernel, session_id, AgentCommand::text(text))
}

pub(crate) fn submit_compaction(
    kernel: &Kernel,
    session_id: SessionId,
) -> renoa_kernel::OperationId {
    submit_command(kernel, session_id, AgentCommand::compact())
}

fn submit_command(
    kernel: &Kernel,
    session_id: SessionId,
    command: AgentCommand,
) -> renoa_kernel::OperationId {
    kernel
        .submit(
            session_id,
            Command::new(
                CommandId::new(),
                serde_json::to_value(command).expect("serialize command"),
            ),
        )
        .expect("submit command")
        .operation_id
}

pub(crate) async fn submit_and_drive(
    kernel: &Kernel,
    session_id: SessionId,
    runtime: &renoa_kernel::Runtime,
    text: &str,
) -> renoa_kernel::OperationId {
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
    operation_id
}

pub(crate) enum Script {
    Response(Box<ModelResponse>),
    ContextOverflow,
    OutcomeUnknown,
    WaitForCancellation(Arc<Notify>),
    Panic,
}

impl Script {
    pub(crate) fn response(response: ModelResponse) -> Self {
        Self::Response(Box::new(response))
    }
}

pub(crate) struct ScriptedModel {
    scripts: Mutex<VecDeque<Script>>,
    requests: Arc<Mutex<Vec<ModelRequest>>>,
}

impl ScriptedModel {
    pub(crate) fn new(
        scripts: impl IntoIterator<Item = Script>,
        requests: Arc<Mutex<Vec<ModelRequest>>>,
    ) -> Self {
        Self {
            scripts: Mutex::new(scripts.into_iter().collect()),
            requests,
        }
    }
}

impl Model for ScriptedModel {
    fn stream(
        &self,
        request: ModelRequest,
        cancellation: CancellationToken,
    ) -> ModelEventStream<'_> {
        self.requests.lock().expect("request lock").push(request);
        let script = self
            .scripts
            .lock()
            .expect("script lock")
            .pop_front()
            .expect("scripted model ran out of responses");
        match script {
            Script::Response(response) => stream::once(async move {
                Ok(ModelEvent::Completed {
                    response: *response,
                })
            })
            .boxed(),
            Script::ContextOverflow => stream::once(async {
                Err(ModelError::context_window_exceeded(
                    "provider rejected the oversized request",
                ))
            })
            .boxed(),
            Script::OutcomeUnknown => {
                stream::once(async { Err(ModelError::new("provider reply was lost")) }).boxed()
            }
            Script::WaitForCancellation(invoked) => stream::once(async move {
                invoked.notify_one();
                cancellation.cancelled().await;
                Err(ModelError::new("summary sampling was cancelled"))
            })
            .boxed(),
            Script::Panic => stream::once(async { panic!("injected model process loss") }).boxed(),
        }
    }
}

pub(crate) struct ThresholdSizer;

impl ContextSizer for ThresholdSizer {
    fn estimate_input_tokens(&self, request: &ModelRequest) -> u64 {
        if request
            .system_prompt
            .starts_with("You create durable context checkpoints")
        {
            return 10;
        }
        request.messages.iter().map(message_cost).sum()
    }
}

pub(crate) struct UnderestimatingSizer;

impl ContextSizer for UnderestimatingSizer {
    fn estimate_input_tokens(&self, _request: &ModelRequest) -> u64 {
        1
    }
}

fn message_cost(message: &renoa_agent::Message) -> u64 {
    let encoded = serde_json::to_string(message).expect("encode message for test sizing");
    if encoded.contains("[CONTEXT CHECKPOINT]") {
        10
    } else {
        20
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

pub(crate) fn tool_response() -> ModelResponse {
    ModelResponse {
        content: vec![AssistantContent::tool_call(ToolCall {
            id: "forbidden-summary-tool".to_owned(),
            name: "read".to_owned(),
            arguments: serde_json::json!({}),
            thought_signature: None,
            namespace: None,
        })],
        stop_reason: StopReason::ToolUse,
        usage: None,
        metadata: AssistantMetadata::default(),
    }
}

pub(crate) fn nz32(value: u32) -> NonZeroU32 {
    NonZeroU32::new(value).expect("test value is non-zero")
}

fn nz64(value: u64) -> NonZeroU64 {
    NonZeroU64::new(value).expect("test value is non-zero")
}
