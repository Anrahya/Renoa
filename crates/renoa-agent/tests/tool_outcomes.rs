use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, Ordering},
};

use renoa_agent::{
    AgentEvent, AgentEventSink, BoxFuture, ContentBlock, Tool, ToolCall, ToolCallBatchError,
    ToolError, ToolOutput, ToolSpec, ToolUpdates, invoke_tool, validate_tool_call_ids,
};
use tokio_util::sync::CancellationToken;

#[tokio::test]
async fn an_uncertain_tool_outcome_stays_unsettled() {
    let call = tool_call("publish-1", "publish");
    let tool = FailingTool::new(
        "publish",
        ToolError::outcome_unknown("connection closed after dispatch"),
    );

    let error = invoke_tool(Some(&tool), call.clone(), CancellationToken::new(), None)
        .await
        .expect_err("an uncertain external action must not become a settled tool result");

    assert_eq!(error.call_id(), call.id);
    assert_eq!(error.tool_name(), call.name);
    assert_eq!(error.message(), "connection closed after dispatch");
    assert!(error.partial_changes_possible());
}

#[tokio::test]
async fn a_definite_tool_failure_becomes_a_model_visible_result() {
    let call = tool_call("build-1", "build");
    let tool = FailingTool::new(
        "build",
        ToolError::process_failed("compiler exited with status 1", true),
    );

    let result = invoke_tool(Some(&tool), call.clone(), CancellationToken::new(), None)
        .await
        .expect("a definite failure is a settled tool result");

    assert_eq!(result.call_id, call.id);
    assert_eq!(result.name, call.name);
    assert!(result.is_error);
    assert_eq!(
        result.content,
        vec![ContentBlock::text("compiler exited with status 1")]
    );
    assert_eq!(
        result.details,
        Some(serde_json::json!({
            "error": {
                "code": "process_failed",
                "partial_changes_possible": true
            }
        }))
    );
}

#[tokio::test]
async fn an_unavailable_tool_becomes_a_typed_model_visible_result() {
    let call = tool_call("missing-1", "missing");

    let result = invoke_tool(None, call.clone(), CancellationToken::new(), None)
        .await
        .expect("an unavailable tool is a definite failure");

    assert!(result.is_error);
    assert_eq!(
        result.details,
        Some(serde_json::json!({
            "error": {
                "code": "unavailable",
                "partial_changes_possible": false
            }
        }))
    );
}

#[tokio::test]
async fn a_pre_cancelled_call_never_invokes_the_tool() {
    let invoked = Arc::new(AtomicBool::new(false));
    let tool = CountingTool::new(Arc::clone(&invoked));
    let cancellation = CancellationToken::new();
    cancellation.cancel();

    let result = invoke_tool(Some(&tool), tool_call("read-1", "read"), cancellation, None)
        .await
        .expect("pre-dispatch cancellation has a definite outcome");

    assert!(!invoked.load(Ordering::SeqCst));
    assert!(result.is_error);
    assert_eq!(
        result.details,
        Some(serde_json::json!({
            "error": {
                "code": "cancelled",
                "partial_changes_possible": false
            }
        }))
    );
}

#[tokio::test]
async fn bounded_progress_is_delivered_in_order_before_the_final_result() {
    let sink = RecordingSink::default();
    let tool = ProgressTool::new();

    let result = invoke_tool(
        Some(&tool),
        tool_call("work-1", "work"),
        CancellationToken::new(),
        Some(&sink),
    )
    .await
    .expect("the tool must settle successfully");

    assert_eq!(result.content, vec![ContentBlock::text("done")]);
    let events = sink.events.lock().expect("event sink lock");
    let updates = events
        .iter()
        .map(|event| match event {
            AgentEvent::ToolExecutionUpdate { update, .. } => update.content.clone(),
            other => panic!("invoke_tool must emit only progress events, got {other:?}"),
        })
        .collect::<Vec<_>>();
    assert_eq!(
        updates,
        vec![
            vec![ContentBlock::text("first")],
            vec![ContentBlock::text("second")]
        ]
    );
}

#[test]
fn tool_call_identifiers_must_be_non_empty_and_unique() {
    assert_eq!(validate_tool_call_ids(["one", "two"]), Ok(()));
    assert_eq!(
        validate_tool_call_ids(["one", ""]),
        Err(ToolCallBatchError::EmptyId)
    );
    assert_eq!(
        validate_tool_call_ids(["one", "one"]),
        Err(ToolCallBatchError::DuplicateId("one".to_owned()))
    );
}

struct FailingTool {
    spec: ToolSpec,
    error: ToolError,
}

impl FailingTool {
    fn new(name: &str, error: ToolError) -> Self {
        Self {
            spec: tool_spec(name),
            error,
        }
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
        Box::pin(std::future::ready(Err(self.error.clone())))
    }
}

struct CountingTool {
    spec: ToolSpec,
    invoked: Arc<AtomicBool>,
}

impl CountingTool {
    fn new(invoked: Arc<AtomicBool>) -> Self {
        Self {
            spec: tool_spec("read"),
            invoked,
        }
    }
}

impl Tool for CountingTool {
    fn spec(&self) -> &ToolSpec {
        &self.spec
    }

    fn execute(
        &self,
        _call: ToolCall,
        _cancellation: CancellationToken,
        _updates: ToolUpdates,
    ) -> BoxFuture<'_, Result<ToolOutput, ToolError>> {
        self.invoked.store(true, Ordering::SeqCst);
        Box::pin(std::future::ready(Ok(ToolOutput {
            content: vec![ContentBlock::text("unexpected")],
            details: None,
        })))
    }
}

struct ProgressTool {
    spec: ToolSpec,
}

impl ProgressTool {
    fn new() -> Self {
        Self {
            spec: tool_spec("work"),
        }
    }
}

impl Tool for ProgressTool {
    fn spec(&self) -> &ToolSpec {
        &self.spec
    }

    fn execute(
        &self,
        _call: ToolCall,
        _cancellation: CancellationToken,
        updates: ToolUpdates,
    ) -> BoxFuture<'_, Result<ToolOutput, ToolError>> {
        Box::pin(async move {
            updates
                .emit(ToolOutput {
                    content: vec![ContentBlock::text("first")],
                    details: None,
                })
                .await;
            updates
                .emit(ToolOutput {
                    content: vec![ContentBlock::text("second")],
                    details: None,
                })
                .await;
            Ok(ToolOutput {
                content: vec![ContentBlock::text("done")],
                details: None,
            })
        })
    }
}

#[derive(Default)]
struct RecordingSink {
    events: Mutex<Vec<AgentEvent>>,
}

impl AgentEventSink for RecordingSink {
    fn emit(&self, event: AgentEvent) -> BoxFuture<'_, ()> {
        Box::pin(async move {
            self.events.lock().expect("event sink lock").push(event);
        })
    }
}

fn tool_spec(name: &str) -> ToolSpec {
    ToolSpec {
        name: name.to_owned(),
        description: "Test tool.".to_owned(),
        input_schema: serde_json::json!({"type": "object"}),
    }
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
