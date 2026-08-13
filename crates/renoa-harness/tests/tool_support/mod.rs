#![allow(
    dead_code,
    reason = "each integration test binary uses a different subset of these shared fixtures"
)]

use std::{collections::VecDeque, sync::Mutex};

use futures_util::stream;
use renoa_agent::{
    BoxFuture, Model, ModelEvent, ModelEventStream, ModelRequest, ModelResponse, TokenUsage, Tool,
    ToolCall, ToolError, ToolOutput, ToolSpec, ToolUpdates,
};
use tokio_util::sync::CancellationToken;

pub(crate) fn usage(input: u64, output: u64) -> TokenUsage {
    TokenUsage {
        input,
        output,
        cache_read: 0,
        cache_write: 0,
    }
}

pub(crate) struct ScriptedModel {
    responses: Mutex<VecDeque<ModelResponse>>,
    requests: Mutex<Vec<ModelRequest>>,
}

impl ScriptedModel {
    pub(crate) fn new(responses: impl IntoIterator<Item = ModelResponse>) -> Self {
        Self {
            responses: Mutex::new(responses.into_iter().collect()),
            requests: Mutex::new(Vec::new()),
        }
    }

    pub(crate) fn requests(&self) -> Vec<ModelRequest> {
        self.requests.lock().expect("request lock").clone()
    }
}

impl Model for ScriptedModel {
    fn stream(
        &self,
        request: ModelRequest,
        _cancellation: CancellationToken,
    ) -> ModelEventStream<'_> {
        self.requests.lock().expect("request lock").push(request);
        let response = self
            .responses
            .lock()
            .expect("response lock")
            .pop_front()
            .expect("scripted response");
        Box::pin(stream::once(std::future::ready(Ok(
            ModelEvent::Completed { response },
        ))))
    }
}

pub(crate) struct RecordingTool {
    spec: ToolSpec,
    output: ToolOutput,
    calls: Mutex<Vec<ToolCall>>,
}

impl RecordingTool {
    pub(crate) fn new(spec: ToolSpec, output: ToolOutput) -> Self {
        Self {
            spec,
            output,
            calls: Mutex::new(Vec::new()),
        }
    }

    pub(crate) fn calls(&self) -> Vec<ToolCall> {
        self.calls.lock().expect("call lock").clone()
    }
}

impl Tool for RecordingTool {
    fn spec(&self) -> &ToolSpec {
        &self.spec
    }

    fn execute(
        &self,
        call: ToolCall,
        _cancellation: CancellationToken,
        _updates: ToolUpdates,
    ) -> BoxFuture<'_, Result<ToolOutput, ToolError>> {
        self.calls.lock().expect("call lock").push(call);
        Box::pin(std::future::ready(Ok(self.output.clone())))
    }
}
