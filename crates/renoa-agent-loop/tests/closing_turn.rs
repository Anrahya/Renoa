//! A tool result that carries a closing message ends the turn: the closing
//! message is the turn's last assistant message, later calls of the same
//! response are not run, and the next command answers it.

use std::{
    collections::VecDeque,
    num::NonZeroU32,
    sync::{Arc, Mutex},
};

use futures_util::{StreamExt, stream};
use renoa_agent::{
    AssistantContent, AssistantMetadata, BoxFuture, Closing, ContentBlock, Message, Model,
    ModelError, ModelEvent, ModelEventStream, ModelRequest, ModelResponse, StopReason, Tool,
    ToolCall, ToolError, ToolOutput, ToolSpec, ToolUpdates,
};
use renoa_agent_loop::{
    AgentCommand, AgentLoopConfig, AgentToolBinding, ContextBinding, ModelBinding, build_runtime,
};
use renoa_kernel::{
    AgentId, Command, CommandId, DriveResult, EffectRecovery, EventCursor, Kernel,
    OperationOutcome, SessionId,
};
use tempfile::tempdir;
use tokio_util::sync::CancellationToken;

const QUESTION: &str = "Book the 6pm table?\n- Yes\n- No";
const NOT_RUN: &str = "Not run: your turn ended when you asked.";

#[tokio::test]
async fn a_closing_result_ends_the_turn_and_the_next_command_answers_it() {
    let directory = tempdir().expect("temporary directory");
    let kernel = Kernel::open(directory.path().join("kernel.sqlite3")).expect("open kernel");
    let agent_id = AgentId::new();
    let session_id = SessionId::new();
    kernel.create_agent(agent_id).expect("create agent");
    kernel
        .create_session(session_id, agent_id)
        .expect("create session");
    let requests = Arc::new(Mutex::new(Vec::new()));
    let model = Arc::new(ScriptedModel {
        responses: Mutex::new(responses()),
        requests: Arc::clone(&requests),
    });
    let runtime = build_runtime(
        AgentLoopConfig::new(
            "Help the owner.",
            NonZeroU32::new(4).expect("model limit"),
            NonZeroU32::new(4).expect("tool limit"),
        ),
        ContextBinding::full_history(),
        ModelBinding::new("scripted-model-v1", model, EffectRecovery::SafeToReplay),
        vec![AgentToolBinding::new(
            "ask-v1",
            Arc::new(Ask::default()),
            EffectRecovery::SafeToReplay,
        )],
    )
    .expect("runtime");

    let asked = submit(&kernel, session_id, "Find us a table tonight.");
    assert_eq!(
        kernel.drive(session_id, &runtime).await.expect("drive"),
        DriveResult::Finished {
            operation_id: asked,
            outcome: OperationOutcome::WaitingForInput,
        }
    );
    let history = messages(&kernel, session_id);
    let [
        _,
        Message::Assistant { .. },
        Message::Tool { result: asked },
        Message::Tool { result: skipped },
        Message::Assistant {
            content, metadata, ..
        },
    ] = history.as_slice()
    else {
        panic!("user, calls, result, not-run, closing: {history:?}");
    };
    assert_eq!(
        (
            asked.call_id.as_str(),
            asked.is_error,
            asked.closing.is_none()
        ),
        ("ask-1", false, true),
        "the first call ran, and the stored result keeps no closing"
    );
    assert_eq!(
        (skipped.call_id.as_str(), skipped.is_error),
        ("ask-2", true)
    );
    assert_eq!(skipped.content, vec![ContentBlock::text(NOT_RUN)]);
    assert_eq!(content, &vec![AssistantContent::text(QUESTION)]);
    assert_eq!(
        metadata,
        &AssistantMetadata {
            response_id: None,
            ..caller()
        },
        "a provider replays the closing message as the caller's own"
    );

    let answered = submit(&kernel, session_id, "Yes");
    assert_eq!(
        kernel.drive(session_id, &runtime).await.expect("drive"),
        DriveResult::Finished {
            operation_id: answered,
            outcome: OperationOutcome::Completed,
        }
    );
    let requests = requests.lock().expect("requests");
    assert_eq!(requests.len(), 2, "the ask ended the first turn");
    let answer_request = &requests[1].messages;
    assert_eq!(answer_request.len(), 6, "{answer_request:?}");
    assert!(matches!(
        &answer_request[4],
        Message::Assistant { content, .. } if content == &vec![AssistantContent::text(QUESTION)]
    ));
    assert_eq!(answer_request[5], Message::user_text("Yes"));
}

/// The asking response, then the answer to the owner's reply.
fn responses() -> VecDeque<ModelResponse> {
    VecDeque::from([
        ModelResponse {
            content: vec![
                AssistantContent::tool_call(call("ask-1")),
                AssistantContent::tool_call(call("ask-2")),
            ],
            stop_reason: StopReason::ToolUse,
            usage: None,
            metadata: caller(),
        },
        ModelResponse {
            content: vec![AssistantContent::text("Booked for six.")],
            stop_reason: StopReason::Stop,
            usage: None,
            metadata: AssistantMetadata::default(),
        },
    ])
}

fn submit(kernel: &Kernel, session_id: SessionId, text: &str) -> renoa_kernel::OperationId {
    let content = serde_json::to_value(AgentCommand::text(text)).expect("command");
    kernel
        .submit(session_id, Command::new(CommandId::new(), content))
        .expect("submit")
        .operation_id
}

fn messages(kernel: &Kernel, session_id: SessionId) -> Vec<Message> {
    kernel
        .events_after(session_id, EventCursor::START)
        .expect("history")
        .events
        .into_iter()
        .filter_map(|event| serde_json::from_value(event.payload).ok())
        .collect()
}

/// The provider an assistant message names, which every one in history must.
fn caller() -> AssistantMetadata {
    AssistantMetadata {
        api: Some("anthropic-messages".to_owned()),
        provider: Some("anthropic".to_owned()),
        model: Some("claude-fixture".to_owned()),
        response_id: Some("response-1".to_owned()),
        ..AssistantMetadata::default()
    }
}

fn call(id: &str) -> ToolCall {
    ToolCall {
        id: id.to_owned(),
        name: "ask".to_owned(),
        arguments: serde_json::json!({}),
        thought_signature: None,
        namespace: None,
    }
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

#[derive(Default)]
struct Ask {
    runs: Mutex<u32>,
}

impl Tool for Ask {
    fn spec(&self) -> &ToolSpec {
        static SPEC: std::sync::OnceLock<ToolSpec> = std::sync::OnceLock::new();
        SPEC.get_or_init(|| ToolSpec {
            name: "ask".to_owned(),
            description: "Ask the owner.".to_owned(),
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
            let mut runs = self.runs.lock().expect("runs");
            *runs += 1;
            assert_eq!(*runs, 1, "a later call of the same response never runs");
            Ok(ToolOutput {
                content: vec![ContentBlock::text("Asked.")],
                details: None,
                is_error: false,
                closing: Some(Closing {
                    message: QUESTION.to_owned(),
                    not_run: NOT_RUN.to_owned(),
                }),
            })
        })
    }
}
