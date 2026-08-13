use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

use futures_util::stream;
use renoa_agent::{
    AssistantContent, AssistantMetadata, BoxFuture, Model, ModelEvent, ModelEventStream,
    ModelRequest, ModelResponse, StopReason, Tool, ToolCall, ToolError, ToolOutput, ToolSpec,
    ToolUpdates,
};
use tokio::sync::Notify;
use tokio_util::sync::CancellationToken;

#[derive(Default)]
pub(super) struct PendingModel {
    pub(super) started: Notify,
}

impl Model for PendingModel {
    fn stream(
        &self,
        _request: ModelRequest,
        _cancellation: CancellationToken,
    ) -> ModelEventStream<'_> {
        self.started.notify_one();
        Box::pin(stream::pending())
    }
}

pub(super) struct NeverCalledModel;

impl Model for NeverCalledModel {
    fn stream(
        &self,
        _request: ModelRequest,
        _cancellation: CancellationToken,
    ) -> ModelEventStream<'_> {
        panic!("cancelled operation must not sample again")
    }
}

pub(super) struct OneResponseModel(pub(super) ModelResponse);

impl Model for OneResponseModel {
    fn stream(
        &self,
        _request: ModelRequest,
        _cancellation: CancellationToken,
    ) -> ModelEventStream<'_> {
        Box::pin(stream::once(std::future::ready(Ok(
            ModelEvent::Completed {
                response: self.0.clone(),
            },
        ))))
    }
}

pub(super) struct CooperativeTool {
    pub(super) started: Notify,
    pub(super) cancellation_seen: Notify,
    pub(super) allow_stop: Notify,
    pub(super) stopped: AtomicBool,
    pub(super) calls: AtomicUsize,
    spec: ToolSpec,
}

impl CooperativeTool {
    pub(super) fn new() -> Self {
        Self {
            started: Notify::new(),
            cancellation_seen: Notify::new(),
            allow_stop: Notify::new(),
            stopped: AtomicBool::new(false),
            calls: AtomicUsize::new(0),
            spec: ToolSpec {
                name: "bash".to_owned(),
                description: "Run one command".to_owned(),
                input_schema: serde_json::json!({"type": "object"}),
            },
        }
    }
}

impl Tool for CooperativeTool {
    fn spec(&self) -> &ToolSpec {
        &self.spec
    }

    fn execute(
        &self,
        _call: ToolCall,
        cancellation: CancellationToken,
        _updates: ToolUpdates,
    ) -> BoxFuture<'_, Result<ToolOutput, ToolError>> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        self.started.notify_one();
        Box::pin(async move {
            cancellation.cancelled().await;
            self.cancellation_seen.notify_one();
            self.allow_stop.notified().await;
            self.stopped.store(true, Ordering::SeqCst);
            Err(ToolError::new("tool stopped"))
        })
    }
}

pub(super) struct NeverCalledTool {
    spec: ToolSpec,
}

impl NeverCalledTool {
    pub(super) fn new() -> Self {
        Self {
            spec: ToolSpec {
                name: "bash".to_owned(),
                description: "Run one command".to_owned(),
                input_schema: serde_json::json!({"type": "object"}),
            },
        }
    }
}

impl Tool for NeverCalledTool {
    fn spec(&self) -> &ToolSpec {
        &self.spec
    }

    fn execute(
        &self,
        _call: ToolCall,
        _cancellation: CancellationToken,
        _updates: ToolUpdates,
    ) -> BoxFuture<'_, Result<ToolOutput, ToolError>> {
        panic!("cancelled pending tool must not replay")
    }
}

pub(super) fn tool_response(calls: impl IntoIterator<Item = ToolCall>) -> ModelResponse {
    ModelResponse {
        content: calls.into_iter().map(AssistantContent::tool_call).collect(),
        stop_reason: StopReason::ToolUse,
        usage: None,
        metadata: AssistantMetadata::default(),
    }
}

pub(super) fn bash_call(id: &str, command: &str) -> ToolCall {
    ToolCall {
        id: id.to_owned(),
        name: "bash".to_owned(),
        arguments: serde_json::json!({"command": command}),
        thought_signature: None,
        namespace: None,
    }
}
