use std::{
    num::NonZeroU32,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
};

use renoa_agent::{
    AssistantContent, AssistantMetadata, BoxFuture, ContentBlock, Message, ModelResponse,
    StopReason, Tool, ToolCall, ToolError, ToolOutput, ToolResult, ToolSpec, ToolUpdates,
};
use renoa_harness::{
    Harness, OperationOutcome, OperationRequest, RequestId, RunNext, RuntimeProfile,
    RuntimeProfileError, SessionId, ToolBinding, ToolRecovery,
};
use tempfile::tempdir;
use tokio_util::sync::CancellationToken;

mod tool_support;

use tool_support::ScriptedModel;

#[test]
fn duplicate_tool_names_are_rejected_before_a_profile_can_run() {
    let model = Arc::new(ScriptedModel::new(Vec::<ModelResponse>::new()));
    let first = Arc::new(FailingTool::new("bash", "unused"));
    let second = Arc::new(FailingTool::new("bash", "unused"));
    let result = RuntimeProfile::new(
        "coding-v1",
        model,
        "Be precise.",
        NonZeroU32::new(1).expect("non-zero attempt limit"),
    )
    .with_tools(
        vec![
            ToolBinding::new(first, ToolRecovery::NeverReplay),
            ToolBinding::new(second, ToolRecovery::SafeToReplay),
        ],
        NonZeroU32::new(2).expect("non-zero tool-call limit"),
    );

    assert!(matches!(
        result,
        Err(RuntimeProfileError::DuplicateToolName(name)) if name == "bash"
    ));
}

#[tokio::test]
async fn a_tool_error_becomes_one_result_and_the_model_can_recover() {
    let directory = tempdir().expect("temporary directory");
    let database = directory.path().join("harness.sqlite3");
    let call = ToolCall {
        id: "call-1".to_owned(),
        name: "write_file".to_owned(),
        arguments: serde_json::json!({"path": "src/lib.rs"}),
        thought_signature: None,
        namespace: None,
    };
    let model = Arc::new(ScriptedModel::new([
        ModelResponse {
            content: vec![AssistantContent::tool_call(call.clone())],
            stop_reason: StopReason::ToolUse,
            usage: None,
            metadata: AssistantMetadata::default(),
        },
        ModelResponse {
            content: vec![AssistantContent::text("used another approach")],
            stop_reason: StopReason::Stop,
            usage: None,
            metadata: AssistantMetadata::default(),
        },
    ]));
    let tool = Arc::new(FailingTool::new("write_file", "workspace is read-only"));
    let profile = RuntimeProfile::new(
        "coding-v1",
        model.clone(),
        "Be precise.",
        NonZeroU32::new(2).expect("non-zero attempt limit"),
    )
    .with_tools(
        vec![ToolBinding::new(tool.clone(), ToolRecovery::NeverReplay)],
        NonZeroU32::new(2).expect("non-zero tool-call limit"),
    )
    .expect("valid tools");
    let harness = Harness::open(&database).expect("open harness");
    let session_id = SessionId::new();
    harness
        .create_standalone_session(session_id)
        .await
        .expect("create session");
    harness
        .admit_standalone(
            session_id,
            OperationRequest::new(RequestId::new(), vec![ContentBlock::text("update it")]),
        )
        .await
        .expect("admit operation");

    assert!(matches!(
        harness
            .run_next(session_id, &profile)
            .await
            .expect("complete operation"),
        RunNext::Finished {
            outcome: OperationOutcome::Completed { ref output, .. },
            ..
        } if output == "used another approach"
    ));
    assert_eq!(tool.call_count(), 1);
    assert_eq!(
        model.requests()[1].messages.last(),
        Some(&Message::Tool {
            result: ToolResult {
                call_id: call.id,
                name: call.name,
                content: vec![ContentBlock::text("workspace is read-only")],
                details: None,
                is_error: true,
            },
        })
    );
}

struct FailingTool {
    spec: ToolSpec,
    message: &'static str,
    calls: AtomicUsize,
}

impl FailingTool {
    fn new(name: &str, message: &'static str) -> Self {
        Self {
            spec: ToolSpec {
                name: name.to_owned(),
                description: "Always fails for this test".to_owned(),
                input_schema: serde_json::json!({"type": "object"}),
            },
            message,
            calls: AtomicUsize::new(0),
        }
    }

    fn call_count(&self) -> usize {
        self.calls.load(Ordering::SeqCst)
    }
}

impl Tool for FailingTool {
    fn spec(&self) -> &ToolSpec {
        &self.spec
    }

    fn execute(
        &self,
        _call: ToolCall,
        _cancellation: CancellationToken,
        _updates: ToolUpdates,
    ) -> BoxFuture<'_, Result<ToolOutput, ToolError>> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        Box::pin(std::future::ready(Err(ToolError::new(self.message))))
    }
}
