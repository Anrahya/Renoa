use std::{
    collections::BTreeMap,
    sync::{
        Mutex,
        atomic::{AtomicUsize, Ordering},
    },
};

use futures_util::{StreamExt, stream};
use renoa_agent::{
    AgentEvent, AgentEventSink, AssistantContent, AssistantDelta, BoxFuture, Model, ModelError,
    ModelErrorKind, ModelEvent, ModelEventStream, ModelRequest, ModelResponse, SamplingError,
    StopReason, sample_model,
};
use serde_json::json;
use tokio_util::sync::CancellationToken;

#[tokio::test]
async fn a_pre_cancelled_sample_never_invokes_the_model() {
    let model = CountingModel::default();
    let cancellation = CancellationToken::new();
    cancellation.cancel();

    let result = sample_model(
        &model,
        ModelRequest {
            system_prompt: "Be precise.".to_owned(),
            messages: Vec::new(),
            tools: Vec::new(),
        },
        cancellation,
        None,
    )
    .await;
    let Err(error) = result else {
        panic!("a pre-cancelled request must not be dispatched");
    };

    assert!(matches!(error, SamplingError::Cancelled));
    assert_eq!(model.invocations.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn context_rejection_is_known_only_before_assistant_output_starts() {
    let request = ModelRequest {
        system_prompt: String::new(),
        messages: Vec::new(),
        tools: Vec::new(),
    };
    let result = sample_model(
        &ContextRejectingModel { emits_delta: false },
        request.clone(),
        CancellationToken::new(),
        None,
    )
    .await;
    let Err(error) = result else {
        panic!("provider must reject context");
    };
    assert!(matches!(
        error,
        SamplingError::Model(error)
            if error.kind() == ModelErrorKind::ContextWindowExceeded
    ));

    let result = sample_model(
        &ContextRejectingModel { emits_delta: true },
        request,
        CancellationToken::new(),
        None,
    )
    .await;
    let Err(error) = result else {
        panic!("provider must fail after output starts");
    };
    assert!(matches!(
        error,
        SamplingError::Model(error) if error.kind() == ModelErrorKind::OutcomeUnknown
    ));
}

#[tokio::test]
async fn authentication_failure_is_known_only_before_assistant_output_starts() {
    let request = ModelRequest {
        system_prompt: String::new(),
        messages: Vec::new(),
        tools: Vec::new(),
    };
    let result = sample_model(
        &AuthenticationRejectingModel { emits_delta: false },
        request.clone(),
        CancellationToken::new(),
        None,
    )
    .await;
    assert!(matches!(
        result,
        Err(SamplingError::Model(error))
            if error.kind() == ModelErrorKind::AuthenticationFailed
    ));

    let result = sample_model(
        &AuthenticationRejectingModel { emits_delta: true },
        request,
        CancellationToken::new(),
        None,
    )
    .await;
    assert!(matches!(
        result,
        Err(SamplingError::Model(error)) if error.kind() == ModelErrorKind::OutcomeUnknown
    ));
}

#[tokio::test]
async fn model_diagnostics_preserve_provider_flow_and_one_correlation_id() {
    let sink = RecordingSink::default();
    let request = ModelRequest {
        system_prompt: "Be precise.".to_owned(),
        messages: Vec::new(),
        tools: Vec::new(),
    };

    sample_model(
        &DiagnosticModel,
        request.clone(),
        CancellationToken::new(),
        Some(&sink),
    )
    .await
    .expect("diagnostic model must complete");

    let events = sink.events.lock().expect("event sink lock");
    assert_eq!(events.len(), 7);
    let AgentEvent::ModelRequestStart {
        invocation_id,
        request: observed,
    } = &events[0]
    else {
        panic!("first diagnostic must start the model request");
    };
    assert_eq!(observed, &request);
    assert!(matches!(
        &events[1],
        AgentEvent::ModelProviderRequest { invocation_id: id, payload }
            if id == invocation_id && payload == &json!({ "wire": "exact" })
    ));
    assert!(matches!(
        &events[2],
        AgentEvent::ModelProviderResponse { invocation_id: id, status: 200, headers }
            if id == invocation_id && headers.get("x-request-id").map(String::as_str) == Some("request-1")
    ));
    assert!(matches!(
        &events[3],
        AgentEvent::ModelRequestChunk { invocation_id: id, content_index: 0, .. }
            if id == invocation_id
    ));
    assert!(matches!(events[4], AgentEvent::MessageStart { .. }));
    assert!(matches!(events[5], AgentEvent::MessageUpdate { .. }));
    assert!(matches!(
        &events[6],
        AgentEvent::ModelRequestEnd { invocation_id: id, .. } if id == invocation_id
    ));
}

#[derive(Default)]
struct CountingModel {
    invocations: AtomicUsize,
}

impl Model for CountingModel {
    fn stream(
        &self,
        _request: ModelRequest,
        _cancellation: CancellationToken,
    ) -> ModelEventStream<'_> {
        self.invocations.fetch_add(1, Ordering::SeqCst);
        stream::pending().boxed()
    }
}

struct ContextRejectingModel {
    emits_delta: bool,
}

impl Model for ContextRejectingModel {
    fn stream(
        &self,
        _request: ModelRequest,
        _cancellation: CancellationToken,
    ) -> ModelEventStream<'_> {
        let mut events = Vec::new();
        if self.emits_delta {
            events.push(Ok(ModelEvent::ContentDelta {
                content_index: 0,
                delta: AssistantDelta::Text {
                    text: "partial".to_owned(),
                },
            }));
        }
        events.push(Err(ModelError::context_window_exceeded(
            "prompt exceeds context window",
        )));
        stream::iter(events).boxed()
    }
}

struct AuthenticationRejectingModel {
    emits_delta: bool,
}

struct DiagnosticModel;

impl Model for DiagnosticModel {
    fn stream(
        &self,
        _request: ModelRequest,
        _cancellation: CancellationToken,
    ) -> ModelEventStream<'_> {
        stream::iter([
            Ok(ModelEvent::ProviderRequest {
                payload: json!({ "wire": "exact" }),
            }),
            Ok(ModelEvent::ProviderResponse {
                status: 200,
                headers: BTreeMap::from([("x-request-id".to_owned(), "request-1".to_owned())]),
            }),
            Ok(ModelEvent::ContentDelta {
                content_index: 0,
                delta: AssistantDelta::Text {
                    text: "Done".to_owned(),
                },
            }),
            Ok(ModelEvent::Completed {
                response: ModelResponse {
                    content: vec![AssistantContent::text("Done")],
                    stop_reason: StopReason::Stop,
                    usage: None,
                    metadata: renoa_agent::AssistantMetadata::default(),
                },
            }),
        ])
        .boxed()
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

impl Model for AuthenticationRejectingModel {
    fn stream(
        &self,
        _request: ModelRequest,
        _cancellation: CancellationToken,
    ) -> ModelEventStream<'_> {
        let mut events = Vec::new();
        if self.emits_delta {
            events.push(Ok(ModelEvent::ContentDelta {
                content_index: 0,
                delta: AssistantDelta::Text {
                    text: "partial".to_owned(),
                },
            }));
        }
        events.push(Err(ModelError::authentication_failed(
            "OAuth refresh failed",
        )));
        stream::iter(events).boxed()
    }
}
