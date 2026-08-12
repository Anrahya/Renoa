use std::sync::{
    Mutex,
    atomic::{AtomicUsize, Ordering},
};

use futures_util::stream;
use renoa_agent::{
    AssistantContent, AssistantMetadata, Model, ModelError, ModelEvent, ModelEventStream,
    ModelRequest, ModelResponse, StopReason, TokenUsage, ToolCall,
};
use tokio::sync::Notify;
use tokio_util::sync::CancellationToken;

use crate::{Harness, SessionId};

pub(super) async fn create_session(harness: &Harness) -> SessionId {
    let session_id = SessionId::new();
    harness
        .create_standalone_session(session_id)
        .await
        .expect("create session");
    session_id
}

pub(super) struct NeverCalledModel;

impl Model for NeverCalledModel {
    fn stream(
        &self,
        _request: ModelRequest,
        _cancellation: CancellationToken,
    ) -> ModelEventStream<'_> {
        panic!("model must not be called")
    }
}

#[derive(Default)]
pub(super) struct PendingRecordingModel {
    requests: Mutex<Vec<ModelRequest>>,
    pub(super) called: Notify,
}

impl PendingRecordingModel {
    pub(super) fn requests(&self) -> Vec<ModelRequest> {
        self.requests.lock().expect("request lock").clone()
    }
}

impl Model for PendingRecordingModel {
    fn stream(
        &self,
        request: ModelRequest,
        _cancellation: CancellationToken,
    ) -> ModelEventStream<'_> {
        self.requests.lock().expect("request lock").push(request);
        self.called.notify_one();
        Box::pin(stream::pending())
    }
}

pub(super) struct FixedResponseModel(pub(super) ModelResponse);

impl Model for FixedResponseModel {
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

#[derive(Default)]
pub(super) struct UnexpectedToolCallModel {
    requests: Mutex<Vec<ModelRequest>>,
}

impl UnexpectedToolCallModel {
    pub(super) fn requests(&self) -> Vec<ModelRequest> {
        self.requests.lock().expect("request lock").clone()
    }
}

impl Model for UnexpectedToolCallModel {
    fn stream(
        &self,
        request: ModelRequest,
        _cancellation: CancellationToken,
    ) -> ModelEventStream<'_> {
        self.requests.lock().expect("request lock").push(request);
        Box::pin(stream::once(std::future::ready(Ok(
            ModelEvent::Completed {
                response: ModelResponse {
                    content: vec![
                        AssistantContent::text("I will use a tool"),
                        AssistantContent::tool_call(ToolCall {
                            id: "call-1".to_owned(),
                            name: "bash".to_owned(),
                            arguments: serde_json::json!({"command": "pwd"}),
                            thought_signature: None,
                            namespace: None,
                        }),
                    ],
                    stop_reason: StopReason::ToolUse,
                    usage: Some(TokenUsage {
                        input: 4,
                        output: 6,
                        cache_read: 0,
                        cache_write: 0,
                    }),
                    metadata: AssistantMetadata::default(),
                },
            },
        ))))
    }
}

#[derive(Default)]
pub(super) struct FailThenCompleteModel {
    calls: AtomicUsize,
    requests: Mutex<Vec<ModelRequest>>,
}

impl FailThenCompleteModel {
    pub(super) fn requests(&self) -> Vec<ModelRequest> {
        self.requests.lock().expect("request lock").clone()
    }
}

impl Model for FailThenCompleteModel {
    fn stream(
        &self,
        request: ModelRequest,
        _cancellation: CancellationToken,
    ) -> ModelEventStream<'_> {
        self.requests.lock().expect("request lock").push(request);
        if self.calls.fetch_add(1, Ordering::SeqCst) == 0 {
            Box::pin(stream::once(std::future::ready(Err(ModelError::new(
                "temporary failure",
            )))))
        } else {
            Box::pin(stream::once(std::future::ready(Ok(
                ModelEvent::Completed {
                    response: completed_response(),
                },
            ))))
        }
    }
}

pub(super) struct FailingModel(pub(super) &'static str);

impl Model for FailingModel {
    fn stream(
        &self,
        _request: ModelRequest,
        _cancellation: CancellationToken,
    ) -> ModelEventStream<'_> {
        Box::pin(stream::once(std::future::ready(Err(ModelError::new(
            self.0,
        )))))
    }
}

pub(super) struct IncompleteStreamModel;

impl Model for IncompleteStreamModel {
    fn stream(
        &self,
        _request: ModelRequest,
        _cancellation: CancellationToken,
    ) -> ModelEventStream<'_> {
        Box::pin(stream::empty())
    }
}

pub(super) struct CompletingModel;

impl Model for CompletingModel {
    fn stream(
        &self,
        _request: ModelRequest,
        _cancellation: CancellationToken,
    ) -> ModelEventStream<'_> {
        Box::pin(stream::once(std::future::ready(Ok(
            ModelEvent::Completed {
                response: completed_response(),
            },
        ))))
    }
}

#[derive(Default)]
pub(super) struct RecordingModel {
    requests: Mutex<Vec<ModelRequest>>,
}

impl RecordingModel {
    pub(super) fn requests(&self) -> Vec<ModelRequest> {
        self.requests.lock().expect("request lock").clone()
    }
}

impl Model for RecordingModel {
    fn stream(
        &self,
        request: ModelRequest,
        _cancellation: CancellationToken,
    ) -> ModelEventStream<'_> {
        self.requests.lock().expect("request lock").push(request);
        Box::pin(stream::once(std::future::ready(Ok(
            ModelEvent::Completed {
                response: completed_response(),
            },
        ))))
    }
}

fn completed_response() -> ModelResponse {
    ModelResponse {
        content: vec![AssistantContent::text("done")],
        stop_reason: StopReason::Stop,
        usage: None,
        metadata: AssistantMetadata::default(),
    }
}

pub(super) fn response_with_usage() -> ModelResponse {
    ModelResponse {
        content: vec![AssistantContent::text("done")],
        stop_reason: StopReason::Stop,
        usage: Some(TokenUsage {
            input: 11,
            output: 3,
            cache_read: 2,
            cache_write: 1,
        }),
        metadata: AssistantMetadata::default(),
    }
}
