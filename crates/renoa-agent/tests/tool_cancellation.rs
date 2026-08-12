use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};

use futures_util::{StreamExt, stream};
use renoa_agent::{
    Agent, AgentError, AgentEvent, AgentEventSink, AgentHandle, AssistantContent, BoxFuture,
    ContentBlock, Message, Model, ModelEvent, ModelEventStream, ModelRequest, ModelResponse,
    StopReason, Tool, ToolCall, ToolError, ToolOutput, ToolResult, ToolSpec, ToolUpdates,
};
use serde_json::json;
use tokio_util::sync::CancellationToken;

#[tokio::test]
async fn cancellation_before_tool_execution_preserves_context_without_invoking_the_tool() {
    let call = ToolCall {
        id: "call-1".to_owned(),
        name: "side_effect".to_owned(),
        arguments: json!({}),
        thought_signature: None,
        namespace: None,
    };
    let model = Arc::new(SingleResponseModel {
        response: ModelResponse {
            content: vec![AssistantContent::tool_call(call.clone())],
            stop_reason: StopReason::ToolUse,
            usage: None,
            metadata: renoa_agent::AssistantMetadata::default(),
        },
    });
    let tool = Arc::new(CountingTool::new());
    let agent = Agent::new(model, "Be careful.")
        .with_tools(vec![tool.clone()])
        .expect("unique tool names must be accepted");
    let handle = agent.handle();
    let mut agent = agent.with_event_sink(Arc::new(AbortOnToolStart { handle }));

    let error = agent
        .prompt("Run the tool")
        .await
        .expect_err("the event listener must cancel the run");

    assert!(matches!(error, AgentError::Cancelled));
    assert_eq!(tool.executions.load(Ordering::SeqCst), 0);
    assert_eq!(
        agent.state().messages(),
        &[
            Message::user_text("Run the tool"),
            Message::Assistant {
                content: vec![AssistantContent::tool_call(call.clone())],
                stop_reason: StopReason::ToolUse,
                usage: None,
                metadata: renoa_agent::AssistantMetadata::default(),
            },
            Message::Tool {
                result: ToolResult {
                    call_id: call.id,
                    name: call.name,
                    content: vec![ContentBlock::text("Tool execution was cancelled.")],
                    details: None,
                    is_error: true,
                },
            },
        ]
    );
}

struct SingleResponseModel {
    response: ModelResponse,
}

impl Model for SingleResponseModel {
    fn stream(
        &self,
        _request: ModelRequest,
        _cancellation: CancellationToken,
    ) -> ModelEventStream<'_> {
        stream::once(async {
            Ok(ModelEvent::Completed {
                response: self.response.clone(),
            })
        })
        .boxed()
    }
}

struct CountingTool {
    spec: ToolSpec,
    executions: AtomicUsize,
}

impl CountingTool {
    fn new() -> Self {
        Self {
            spec: ToolSpec {
                name: "side_effect".to_owned(),
                description: "Records execution.".to_owned(),
                input_schema: json!({ "type": "object" }),
            },
            executions: AtomicUsize::new(0),
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
        self.executions.fetch_add(1, Ordering::SeqCst);
        Box::pin(async {
            Ok(ToolOutput {
                content: vec![ContentBlock::text("executed")],
                details: None,
            })
        })
    }
}

struct AbortOnToolStart {
    handle: AgentHandle,
}

impl AgentEventSink for AbortOnToolStart {
    fn emit(&self, event: AgentEvent) -> BoxFuture<'_, ()> {
        Box::pin(async move {
            if matches!(event, AgentEvent::ToolExecutionStart { .. }) {
                self.handle.abort();
            }
        })
    }
}
