use std::sync::{
    Arc, Mutex, OnceLock,
    atomic::{AtomicUsize, Ordering},
};

use renoa_agent::{
    AssistantContent, AssistantMetadata, BoxFuture, ContentBlock, Message, ModelResponse,
    StopReason, Tool, ToolCall, ToolError, ToolOutput, ToolSpec, ToolUpdates,
};
use renoa_agent_loop::{AgentToolBinding, CONTEXT_CHECKPOINT_EVENT_KIND, MESSAGE_EVENT_KIND};
use renoa_kernel::{EffectRecovery, EventCursor, Kernel};
use tempfile::tempdir;
use tokio_util::sync::CancellationToken;

use super::compaction_support::{
    SUMMARY, Script, ScriptedModel, ThresholdSizer, compacting_strategy, create_session, nz32,
    runtime_with_context_and_tools, submit_and_drive, text_response,
};

#[tokio::test]
async fn repeated_compaction_inside_a_tool_turn_keeps_the_exact_user_anchor() {
    let directory = tempdir().expect("temporary directory");
    let kernel = Kernel::open(directory.path().join("kernel.sqlite3")).expect("open kernel");
    let session_id = create_session(&kernel);
    let requests = Arc::new(Mutex::new(Vec::new()));
    let model = Arc::new(ScriptedModel::new(
        [
            Script::response(tool_call_response("call-1")),
            Script::response(text_response(SUMMARY)),
            Script::response(tool_call_response("call-2")),
            Script::response(text_response(SUMMARY)),
            Script::response(text_response("Finished both tools.")),
        ],
        Arc::clone(&requests),
    ));
    let tool_calls = Arc::new(AtomicUsize::new(0));
    let tool = Arc::new(CountingTool {
        calls: Arc::clone(&tool_calls),
    });
    let context = Arc::new(compacting_strategy(Arc::new(ThresholdSizer), nz32(2)));
    let runtime = runtime_with_context_and_tools(
        model,
        context,
        "repeated-active-turn-v1",
        vec![AgentToolBinding::new(
            "counting-tool-v1",
            tool,
            EffectRecovery::SafeToReplay,
        )],
    );

    submit_and_drive(&kernel, session_id, &runtime, "Work through both tools.").await;

    assert_eq!(tool_calls.load(Ordering::SeqCst), 2);
    let requests = requests.lock().expect("request lock");
    assert_eq!(requests.len(), 5);
    assert!(requests[1].tools.is_empty());
    assert!(requests[3].tools.is_empty());
    let second_summary =
        serde_json::to_string(&requests[3]).expect("encode second summary request");
    assert!(second_summary.contains("previous_checkpoint"));
    assert!(second_summary.contains("call-2"));
    let final_request = &requests[4];
    assert_eq!(final_request.messages.len(), 2);
    assert!(checkpoint_text(&final_request.messages[0]).contains(SUMMARY));
    assert_eq!(
        final_request.messages[1],
        Message::user_text("Work through both tools.")
    );
    drop(requests);

    let events = kernel
        .events_after(session_id, EventCursor::START)
        .expect("read durable journal")
        .events;
    let checkpoints = events
        .iter()
        .filter(|event| event.kind == CONTEXT_CHECKPOINT_EVENT_KIND)
        .collect::<Vec<_>>();
    assert_eq!(checkpoints.len(), 2);
    assert_eq!(checkpoints[0].payload["covered_through_sequence"], 2);
    assert_eq!(checkpoints[1].payload["covered_through_sequence"], 5);
    assert_eq!(
        events
            .iter()
            .filter(|event| event.kind == MESSAGE_EVENT_KIND)
            .count(),
        6
    );
}

struct CountingTool {
    calls: Arc<AtomicUsize>,
}

impl Tool for CountingTool {
    fn spec(&self) -> &ToolSpec {
        static SPEC: OnceLock<ToolSpec> = OnceLock::new();
        SPEC.get_or_init(|| ToolSpec {
            name: "count".to_owned(),
            description: "Return the call ordinal.".to_owned(),
            input_schema: serde_json::json!({"type": "object"}),
        })
    }

    fn execute(
        &self,
        _call: ToolCall,
        _cancellation: CancellationToken,
        _updates: ToolUpdates,
    ) -> BoxFuture<'_, Result<ToolOutput, ToolError>> {
        let ordinal = self.calls.fetch_add(1, Ordering::SeqCst) + 1;
        Box::pin(std::future::ready(Ok(ToolOutput {
            content: vec![ContentBlock::text(format!("tool result {ordinal}"))],
            details: None,
            is_error: false,
        })))
    }
}

fn tool_call_response(id: &str) -> ModelResponse {
    ModelResponse {
        content: vec![AssistantContent::tool_call(ToolCall {
            id: id.to_owned(),
            name: "count".to_owned(),
            arguments: serde_json::json!({}),
            thought_signature: None,
            namespace: None,
        })],
        stop_reason: StopReason::ToolUse,
        usage: None,
        metadata: AssistantMetadata::default(),
    }
}

fn checkpoint_text(message: &Message) -> &str {
    let Message::User { content } = message else {
        panic!("checkpoint must be a user message");
    };
    let [ContentBlock::Text { text }] = content.as_slice() else {
        panic!("checkpoint must contain one text block");
    };
    text
}
