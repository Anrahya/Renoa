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
    AgentCommand, AgentLoopConfig, AgentToolBinding, ContextBinding, ContextContribution,
    ModelBinding, TurnContext, build_runtime,
};
use renoa_kernel::{
    AgentId, Command, CommandId, DriveResult, EffectRecovery, Kernel, OperationOutcome, Runtime,
    SessionId, SteerState,
};
use tempfile::tempdir;
use tokio::sync::Notify;
use tokio_util::sync::CancellationToken;

fn observed(text: &str, at: i64, context: &str) -> serde_json::Value {
    let context = TurnContext::new(vec![
        ContextContribution::surface(context).expect("surface context"),
    ])
    .expect("turn context");
    serde_json::to_value(
        AgentCommand::observed(vec![ContentBlock::text(text)], at, context)
            .expect("observed command"),
    )
    .expect("encode command")
}

fn user_text(message: &Message) -> String {
    let Message::User { content } = message else {
        panic!("expected a user message, got {message:?}");
    };
    content
        .iter()
        .filter_map(|block| match block {
            ContentBlock::Text { text } => Some(text.as_str()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("\n")
}

#[tokio::test]
async fn a_steer_sent_during_a_tool_call_reaches_the_next_model_request() {
    let directory = tempdir().expect("temporary directory");
    let kernel = Kernel::open(directory.path().join("kernel.sqlite3")).expect("open kernel");
    let session_id = session(&kernel);
    let requests = Arc::new(Mutex::new(Vec::new()));
    let started = Arc::new(Notify::new());
    let release = Arc::new(Notify::new());
    let runtime = runtime(
        [tool_response(), text_response("Shorter it is.")],
        &requests,
        GatedTool {
            started: Arc::clone(&started),
            release: Arc::clone(&release),
        },
    );
    let anchor = Command::new(
        CommandId::new(),
        observed("Make a video.", 1_000, "anchor-context"),
    );
    let anchor_id = anchor.command_id();
    kernel
        .submit_exclusive(session_id, anchor)
        .expect("submit anchor");
    let steer = Command::new(
        CommandId::new(),
        observed("Keep it under a minute.", 2_000, "steer-context"),
    );

    let (result, ()) = tokio::join!(kernel.drive(session_id, &runtime), async {
        started.notified().await;
        kernel
            .request_steer(session_id, anchor_id, steer.clone())
            .expect("record steer");
        release.notify_one();
    });

    assert!(matches!(
        result.expect("drive"),
        DriveResult::Finished {
            outcome: OperationOutcome::Completed,
            ..
        }
    ));
    let requests = requests.lock().expect("requests");
    assert_eq!(
        requests.len(),
        2,
        "the steer starts no model call of its own"
    );
    let second = &requests[1].messages;
    assert_eq!(second.len(), 4);
    let anchor_text = user_text(&second[0]);
    assert!(anchor_text.starts_with("Make a video."));
    assert!(anchor_text.contains("anchor-context"));
    assert!(!anchor_text.contains("steer-context"));
    assert!(matches!(second[1], Message::Assistant { .. }));
    assert!(matches!(second[2], Message::Tool { .. }));
    let steer_text = user_text(&second[3]);
    assert!(steer_text.starts_with("Keep it under a minute."));
    assert!(steer_text.contains("steer-context"));
    assert!(!steer_text.contains("anchor-context"));
    drop(requests);
    assert_eq!(
        kernel
            .request_steer(session_id, anchor_id, steer)
            .expect("retry"),
        SteerState::Accepted
    );
}

#[tokio::test]
async fn a_receipted_operation_takes_no_steer() {
    let directory = tempdir().expect("temporary directory");
    let kernel = Kernel::open(directory.path().join("kernel.sqlite3")).expect("open kernel");
    let session_id = session(&kernel);
    let requests = Arc::new(Mutex::new(Vec::new()));
    let release = Arc::new(Notify::new());
    release.notify_one();
    let runtime = runtime(
        [text_response("Done.")],
        &requests,
        GatedTool {
            started: Arc::new(Notify::new()),
            release,
        },
    );
    let anchor = Command::new(
        CommandId::new(),
        serde_json::to_value(AgentCommand::receipted(vec![ContentBlock::text("nightly")]))
            .expect("encode"),
    );
    let anchor_id = anchor.command_id();
    kernel.submit_exclusive(session_id, anchor).expect("submit");
    let steer = Command::new(CommandId::new(), observed("also this", 2_000, "ctx"));
    kernel
        .request_steer(session_id, anchor_id, steer.clone())
        .expect("record steer");

    kernel.drive(session_id, &runtime).await.expect("drive");

    assert_eq!(requests.lock().expect("requests")[0].messages.len(), 1);
    assert_eq!(
        kernel
            .request_steer(session_id, anchor_id, steer)
            .expect("retry"),
        SteerState::Released
    );
}

fn session(kernel: &Kernel) -> SessionId {
    let agent_id = AgentId::new();
    let session_id = SessionId::new();
    kernel.create_agent(agent_id).expect("create agent");
    kernel
        .create_session(session_id, agent_id)
        .expect("create session");
    session_id
}

fn runtime(
    responses: impl IntoIterator<Item = ModelResponse>,
    requests: &Arc<Mutex<Vec<ModelRequest>>>,
    tool: GatedTool,
) -> Runtime {
    build_runtime(
        AgentLoopConfig::new(
            "Help.",
            NonZeroU32::new(4).expect("model limit"),
            NonZeroU32::new(4).expect("tool limit"),
        ),
        ContextBinding::full_history(),
        ModelBinding::new(
            "scripted-model-v1",
            Arc::new(ScriptedModel {
                responses: Mutex::new(responses.into_iter().collect()),
                requests: Arc::clone(requests),
            }),
            EffectRecovery::SafeToReplay,
        ),
        vec![AgentToolBinding::new(
            "render-v1",
            Arc::new(tool),
            EffectRecovery::NeverReplay,
        )],
    )
    .expect("build runtime")
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

/// Signals `started`, then runs until `release`.
struct GatedTool {
    started: Arc<Notify>,
    release: Arc<Notify>,
}

impl Tool for GatedTool {
    fn spec(&self) -> &ToolSpec {
        static SPEC: std::sync::OnceLock<ToolSpec> = std::sync::OnceLock::new();
        SPEC.get_or_init(|| ToolSpec {
            name: "render".to_owned(),
            description: "Render a video.".to_owned(),
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
            self.started.notify_one();
            self.release.notified().await;
            Ok(ToolOutput {
                content: vec![ContentBlock::text("Rendered.")],
                details: None,
                is_error: false,
                closing: None,
            })
        })
    }
}

fn tool_response() -> ModelResponse {
    ModelResponse {
        content: vec![AssistantContent::tool_call(ToolCall {
            id: "render-1".to_owned(),
            name: "render".to_owned(),
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
