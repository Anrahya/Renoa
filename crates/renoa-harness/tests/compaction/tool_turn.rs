use std::{
    num::{NonZeroU32, NonZeroU64},
    sync::{
        Arc, Mutex,
        atomic::{AtomicUsize, Ordering},
    },
};

use futures_util::{StreamExt, stream};
use renoa_agent::{
    AssistantContent, AssistantMetadata, ContentBlock, Message, Model, ModelEvent,
    ModelEventStream, ModelRequest, ModelResponse, StopReason, ToolCall, ToolOutput, ToolSpec,
};
use renoa_harness::{
    CompactionPolicy, ContextSizer, Harness, OperationRequest, RequestId, RuntimeProfile,
    SessionId, ToolBinding, ToolRecovery,
};
use tempfile::tempdir;
use tokio_util::sync::CancellationToken;

use super::{tool_support::RecordingTool, valid_summary};

#[tokio::test]
async fn repeated_compaction_inside_an_active_turn_keeps_its_original_user_anchor() {
    let directory = tempdir().expect("temporary directory");
    let model = Arc::new(ToolTurnModel::default());
    let tool_text = format!("HEAD:{}:TAIL", "x".repeat(100_000));
    let tool = Arc::new(RecordingTool::new(
        ToolSpec {
            name: "read_file".to_owned(),
            description: "Read one file".to_owned(),
            input_schema: serde_json::json!({"type": "object"}),
        },
        ToolOutput {
            content: vec![ContentBlock::text(tool_text.clone())],
            details: None,
        },
    ));
    let profile = RuntimeProfile::new(
        "compact-tools-v1",
        model.clone(),
        "Be precise.",
        NonZeroU32::new(4).expect("non-zero model attempt limit"),
    )
    .with_compaction(
        CompactionPolicy::new(
            NonZeroU64::new(100).expect("non-zero context window"),
            20,
            NonZeroU64::new(50).expect("non-zero target"),
            NonZeroU64::new(40).expect("non-zero summary limit"),
            NonZeroU32::new(2).expect("non-zero compaction attempt limit"),
        )
        .expect("valid compaction policy"),
        Arc::new(ToolTurnSizer),
    )
    .with_tools(
        vec![ToolBinding::new(
            "read-file-v1",
            tool,
            ToolRecovery::SafeToReplay,
        )],
        NonZeroU32::new(1).expect("non-zero tool-call limit"),
    )
    .expect("valid tool binding");
    let harness = Harness::open(directory.path().join("harness.sqlite3")).expect("open harness");
    let session_id = SessionId::new();
    harness
        .create_standalone_session(session_id)
        .await
        .expect("create session");
    harness
        .admit_standalone(
            session_id,
            OperationRequest::new(RequestId::new(), vec![ContentBlock::text("fix it")]),
        )
        .await
        .expect("admit operation");

    harness
        .run_next(session_id, &profile)
        .await
        .expect("run tool operation");

    let requests = model.requests();
    assert_eq!(requests.len(), 5);
    assert_eq!(requests[0].messages, vec![Message::user_text("fix it")]);
    assert!(requests[1].tools.is_empty(), "compactor cannot call tools");
    assert!(requests[3].tools.is_empty(), "compactor cannot call tools");
    for request in [&requests[1], &requests[3]] {
        let compactor_input = serde_json::to_string(request).expect("encode compactor input");
        assert!(compactor_input.len() < 25_000);
        assert!(compactor_input.contains("HEAD:"));
        assert!(compactor_input.contains(":TAIL"));
        assert!(compactor_input.contains("\\\"head\\\""));
        assert!(compactor_input.contains("\\\"tail\\\""));
        assert!(compactor_input.contains("omitted_chars"));
        assert!(compactor_input.contains("tool_result_sha256"));
    }
    assert_eq!(requests[4].messages.len(), 2);
    assert_eq!(requests[4].messages[1], Message::user_text("fix it"));
    let final_context = serde_json::to_string(&requests[4]).expect("encode final context");
    assert!(final_context.contains("CONTEXT CHECKPOINT"));
    assert!(!final_context.contains("HEAD:"));

    let snapshot = harness.inspect(session_id).await.expect("inspect session");
    assert_eq!(snapshot.messages.len(), 6);
    assert!(matches!(snapshot.messages[1], Message::Assistant { .. }));
    assert!(matches!(
        &snapshot.messages[2],
        Message::Tool { result }
            if result.content == vec![ContentBlock::text(tool_text)]
    ));
    assert!(matches!(snapshot.messages[3], Message::Assistant { .. }));
    assert!(matches!(snapshot.messages[4], Message::Tool { .. }));
    assert!(matches!(snapshot.messages[5], Message::Assistant { .. }));
}

struct ToolTurnSizer;

impl ContextSizer for ToolTurnSizer {
    fn estimate_input_tokens(&self, request: &ModelRequest) -> u64 {
        if request.system_prompt != "Be precise." {
            40
        } else if request
            .messages
            .iter()
            .any(|message| matches!(message, Message::Tool { .. }))
        {
            90
        } else if request.messages.iter().any(is_checkpoint) {
            30
        } else {
            10
        }
    }
}

fn is_checkpoint(message: &Message) -> bool {
    serde_json::to_string(message)
        .expect("encode message")
        .contains("CONTEXT CHECKPOINT")
}

#[derive(Default)]
struct ToolTurnModel {
    requests: Mutex<Vec<ModelRequest>>,
    normal_calls: AtomicUsize,
}

impl ToolTurnModel {
    fn requests(&self) -> Vec<ModelRequest> {
        self.requests.lock().expect("request lock").clone()
    }
}

impl Model for ToolTurnModel {
    fn stream(
        &self,
        request: ModelRequest,
        _cancellation: CancellationToken,
    ) -> ModelEventStream<'_> {
        let is_compaction = request.system_prompt != "Be precise.";
        self.requests.lock().expect("request lock").push(request);
        let response = if is_compaction {
            ModelResponse {
                content: vec![AssistantContent::text(valid_summary())],
                stop_reason: StopReason::Stop,
                usage: None,
                metadata: AssistantMetadata::default(),
            }
        } else if self.normal_calls.fetch_add(1, Ordering::Relaxed) == 2 {
            ModelResponse {
                content: vec![AssistantContent::text("implemented")],
                stop_reason: StopReason::Stop,
                usage: None,
                metadata: AssistantMetadata::default(),
            }
        } else {
            let call = self.normal_calls.load(Ordering::Relaxed);
            ModelResponse {
                content: vec![AssistantContent::tool_call(ToolCall {
                    id: format!("call-{call}"),
                    name: "read_file".to_owned(),
                    arguments: serde_json::json!({"path": "src/lib.rs"}),
                    thought_signature: None,
                    namespace: None,
                })],
                stop_reason: StopReason::ToolUse,
                usage: None,
                metadata: AssistantMetadata::default(),
            }
        };
        stream::once(async move { Ok(ModelEvent::Completed { response }) }).boxed()
    }
}
