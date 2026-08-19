use std::sync::{
    Arc, Mutex,
    atomic::{AtomicUsize, Ordering},
};

use futures_util::stream;
use renoa_agent::{
    Agent, AgentError, AssistantContent, AssistantMetadata, BoxFuture, Message, Model, ModelEvent,
    ModelEventStream, ModelRequest, ModelResponse, StopReason, Tool, ToolCall, ToolCallBatchError,
    ToolError, ToolOutput, ToolSpec, ToolUpdates,
};
use tokio_util::sync::CancellationToken;

#[tokio::test]
async fn duplicate_tool_call_identifiers_are_rejected_without_execution() {
    let duplicate_id = "call-duplicate";
    let calls = ["first.txt", "second.txt"].map(|path| ToolCall {
        id: duplicate_id.to_owned(),
        name: "read_file".to_owned(),
        arguments: serde_json::json!({"path": path}),
        thought_signature: None,
        namespace: None,
    });
    let model = Arc::new(SingleResponseModel::new(ModelResponse {
        content: calls.into_iter().map(AssistantContent::tool_call).collect(),
        stop_reason: StopReason::ToolUse,
        usage: None,
        metadata: AssistantMetadata::default(),
    }));
    let mut agent = Agent::new(model.clone(), "Be precise.")
        .with_tools(vec![Arc::new(NeverCalledTool)])
        .expect("unique tool names must be accepted");

    let error = agent
        .prompt("Read both files")
        .await
        .expect_err("duplicate call identifiers must fail the model turn");

    assert!(matches!(
        error,
        AgentError::InvalidToolCallBatch {
            source: ToolCallBatchError::DuplicateId(id),
            usage: None,
        } if id == duplicate_id
    ));
    assert_eq!(model.call_count(), 1);
    assert_eq!(
        agent.state().messages(),
        &[Message::user_text("Read both files")]
    );
}

struct SingleResponseModel {
    response: Mutex<Option<ModelResponse>>,
    calls: AtomicUsize,
}

impl SingleResponseModel {
    fn new(response: ModelResponse) -> Self {
        Self {
            response: Mutex::new(Some(response)),
            calls: AtomicUsize::new(0),
        }
    }

    fn call_count(&self) -> usize {
        self.calls.load(Ordering::SeqCst)
    }
}

impl Model for SingleResponseModel {
    fn stream(
        &self,
        _request: ModelRequest,
        _cancellation: CancellationToken,
    ) -> ModelEventStream<'_> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        let response = self
            .response
            .lock()
            .expect("response lock")
            .take()
            .expect("single response");
        Box::pin(stream::once(std::future::ready(Ok(
            ModelEvent::Completed { response },
        ))))
    }
}

struct NeverCalledTool;

impl Tool for NeverCalledTool {
    fn spec(&self) -> &ToolSpec {
        static SPEC: std::sync::OnceLock<ToolSpec> = std::sync::OnceLock::new();
        SPEC.get_or_init(|| ToolSpec {
            name: "read_file".to_owned(),
            description: "Must not execute for an invalid tool-call batch.".to_owned(),
            input_schema: serde_json::json!({"type": "object"}),
        })
    }

    fn execute(
        &self,
        _call: ToolCall,
        _cancellation: CancellationToken,
        _updates: ToolUpdates,
    ) -> BoxFuture<'_, Result<ToolOutput, ToolError>> {
        panic!("invalid tool-call batches must fail before tool execution")
    }
}
