use std::{
    collections::VecDeque,
    num::NonZeroU32,
    sync::{Arc, Mutex},
};

use futures_util::{StreamExt, stream};
use renoa_agent::{
    AssistantContent, AssistantMetadata, BoxFuture, ContentBlock, Message, Model, ModelError,
    ModelEvent, ModelEventStream, ModelRequest, ModelResponse, StopReason, Tool, ToolCall,
    ToolError, ToolOutput, ToolSpec, ToolUpdates,
};
use renoa_agent_loop::{
    AgentCommand, AgentLoopConfig, AgentToolBinding, ContextBinding, ModelBinding, build_runtime,
};
use renoa_kernel::{AgentId, Command, CommandId, EffectRecovery, Kernel, SessionId};
use tempfile::tempdir;
use tokio_util::sync::CancellationToken;

#[tokio::test]
async fn three_quiet_tool_steps_remind_the_model_to_say_how_it_goes() {
    let directory = tempdir().expect("temporary directory");
    let kernel = Kernel::open(directory.path().join("kernel.sqlite3")).expect("open kernel");
    let agent_id = AgentId::new();
    let session_id = SessionId::new();
    kernel.create_agent(agent_id).expect("create agent");
    kernel
        .create_session(session_id, agent_id)
        .expect("create session");
    let requests = Arc::new(Mutex::new(Vec::new()));
    let responses = [
        tool_response("check-1"),
        tool_response("check-2"),
        tool_response("check-3"),
        text_response("The clips are still rendering."),
    ];
    let runtime = build_runtime(
        AgentLoopConfig::new(
            "Help.",
            NonZeroU32::new(8).expect("model limit"),
            NonZeroU32::new(8).expect("tool limit"),
        ),
        ContextBinding::full_history(),
        ModelBinding::new(
            "scripted-model-v1",
            Arc::new(ScriptedModel {
                responses: Mutex::new(responses.into_iter().collect()),
                requests: Arc::clone(&requests),
            }),
            EffectRecovery::SafeToReplay,
        ),
        vec![AgentToolBinding::new(
            "check-v1",
            Arc::new(Check),
            EffectRecovery::SafeToReplay,
        )],
    )
    .expect("build runtime");
    let prompt =
        serde_json::to_value(AgentCommand::text("Render the reel.")).expect("encode command");
    kernel
        .submit_exclusive(session_id, Command::new(CommandId::new(), prompt))
        .expect("submit prompt");

    kernel
        .drive(session_id, &runtime)
        .await
        .expect("drive the turn");

    let requests = requests.lock().expect("requests");
    let reminded = |request: &ModelRequest| {
        request
            .messages
            .iter()
            .filter(|message| {
                matches!(message, Message::Tool { result }
                    if result.content.iter().any(|block| matches!(block,
                        ContentBlock::Text { text } if text.starts_with("<reminder>"))))
            })
            .count()
    };
    assert_eq!(
        requests.iter().map(reminded).collect::<Vec<_>>(),
        vec![0, 0, 0, 1],
        "only the request after the third quiet step carries the reminder"
    );
}

struct ScriptedModel {
    responses: Mutex<VecDeque<ModelResponse>>,
    requests: Arc<Mutex<Vec<ModelRequest>>>,
}

impl Model for ScriptedModel {
    fn stream(
        &self,
        request: ModelRequest,
        _cancellation: CancellationToken,
    ) -> ModelEventStream<'_> {
        self.requests.lock().expect("requests").push(request);
        let response = self
            .responses
            .lock()
            .expect("responses")
            .pop_front()
            .ok_or_else(|| ModelError::new("scripted model ran out of responses"));
        stream::once(async move { response.map(|response| ModelEvent::Completed { response }) })
            .boxed()
    }
}

struct Check;

impl Tool for Check {
    fn spec(&self) -> &ToolSpec {
        static SPEC: std::sync::OnceLock<ToolSpec> = std::sync::OnceLock::new();
        SPEC.get_or_init(|| ToolSpec {
            name: "check".to_owned(),
            description: "Check the render.".to_owned(),
            input_schema: serde_json::json!({"type": "object"}),
        })
    }

    fn execute(
        &self,
        _call: ToolCall,
        _cancellation: CancellationToken,
        _updates: ToolUpdates,
    ) -> BoxFuture<'_, Result<ToolOutput, ToolError>> {
        Box::pin(async move {
            Ok(ToolOutput {
                content: vec![ContentBlock::text("Still rendering.")],
                details: None,
                is_error: false,
                closing: None,
            })
        })
    }
}

fn tool_response(id: &str) -> ModelResponse {
    ModelResponse {
        content: vec![AssistantContent::tool_call(ToolCall {
            id: id.to_owned(),
            name: "check".to_owned(),
            arguments: serde_json::json!({}),
            thought_signature: None,
            namespace: None,
        })],
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
