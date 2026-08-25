use std::{
    collections::BTreeMap,
    sync::{
        Mutex,
        atomic::{AtomicUsize, Ordering},
    },
};

use futures_util::{StreamExt, stream};
use renoa_agent::{
    AgentEvent, AgentEventSink, AssistantContent, AssistantDelta, AssistantMetadata, BoxFuture,
    InferenceOutcome, Model, ModelError, ModelErrorKind, ModelEvent, ModelEventStream,
    ModelRequest, ModelResponse, SamplingError, StopReason, sample_model,
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
async fn cancelling_before_provider_dispatch_is_cancelled() {
    let sink = RecordingSink::default();
    let cancellation = CancellationToken::new();
    let request = ModelRequest {
        system_prompt: "Be precise.".to_owned(),
        messages: Vec::new(),
        tools: Vec::new(),
    };
    let sampling = sample_model(
        &HangBeforeDispatchModel,
        request,
        cancellation.clone(),
        Some(&sink),
    );
    let wait = async {
        loop {
            {
                let events = sink.events.lock().expect("event sink lock");
                if events
                    .iter()
                    .any(|event| matches!(event, AgentEvent::ModelRequestStart { .. }))
                {
                    break;
                }
            }
            tokio::task::yield_now().await;
        }
        cancellation.cancel();
    };
    let (result, ()) = tokio::join!(sampling, wait);
    let Err(error) = result else {
        panic!("pre-dispatch cancellation must fail");
    };
    assert!(matches!(error, SamplingError::Cancelled));
}

#[tokio::test]
async fn cancelling_after_provider_dispatch_is_unknown() {
    let sink = RecordingSink::default();
    let cancellation = CancellationToken::new();
    let request = ModelRequest {
        system_prompt: "Be precise.".to_owned(),
        messages: Vec::new(),
        tools: Vec::new(),
    };
    let sampling = sample_model(
        &HangAfterDispatchModel,
        request,
        cancellation.clone(),
        Some(&sink),
    );
    let wait = async {
        loop {
            {
                let events = sink.events.lock().expect("event sink lock");
                if events
                    .iter()
                    .any(|event| matches!(event, AgentEvent::ModelProviderRequest { .. }))
                {
                    break;
                }
            }
            tokio::task::yield_now().await;
        }
        cancellation.cancel();
    };
    let (result, ()) = tokio::join!(sampling, wait);
    let Err(error) = result else {
        panic!("in-flight cancellation must fail");
    };
    assert!(matches!(
        error,
        SamplingError::Model(error)
            if error.kind() == ModelErrorKind::Cancelled
                && error.inference_outcome() == InferenceOutcome::Unknown
    ));
}

#[tokio::test]
async fn a_completed_response_survives_cancellation_drain() {
    let sink = RecordingSink::default();
    let cancellation = CancellationToken::new();
    let request = ModelRequest {
        system_prompt: "Be precise.".to_owned(),
        messages: Vec::new(),
        tools: Vec::new(),
    };
    let sampling = sample_model(
        &CompletesAfterCancelModel,
        request,
        cancellation.clone(),
        Some(&sink),
    );
    let wait = async {
        loop {
            {
                let events = sink.events.lock().expect("event sink lock");
                if events
                    .iter()
                    .any(|event| matches!(event, AgentEvent::ModelProviderRequest { .. }))
                {
                    break;
                }
            }
            tokio::task::yield_now().await;
        }
        cancellation.cancel();
    };
    let (result, ()) = tokio::join!(sampling, wait);
    let sampled = result.expect("completed provider result must survive cancellation");
    assert_eq!(
        sampled.response.content,
        vec![AssistantContent::text("definite")]
    );
    let events = sink.events.lock().expect("event sink lock");
    assert!(
        events
            .iter()
            .any(|event| matches!(event, AgentEvent::ModelRequestEnd { .. }))
    );
    assert!(
        !events
            .iter()
            .any(|event| matches!(event, AgentEvent::ModelRequestFailed { .. })),
        "one invocation must not emit both ModelRequestEnd and ModelRequestFailed: {events:?}"
    );
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
        SamplingError::Model(error)
            if error.kind() == ModelErrorKind::ContextWindowExceeded
                && error.inference_outcome() == InferenceOutcome::Unknown
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
            if error.kind() == ModelErrorKind::Authentication
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
        Err(SamplingError::Model(error))
            if error.kind() == ModelErrorKind::Authentication
                && error.inference_outcome() == InferenceOutcome::Unknown
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

struct HangBeforeDispatchModel;

impl Model for HangBeforeDispatchModel {
    fn stream(
        &self,
        _request: ModelRequest,
        cancellation: CancellationToken,
    ) -> ModelEventStream<'_> {
        stream::once(async move {
            cancellation.cancelled().await;
            Err(ModelError::new("cancelled model stopped"))
        })
        .boxed()
    }
}

struct CompletesAfterCancelModel;

impl Model for CompletesAfterCancelModel {
    fn stream(
        &self,
        _request: ModelRequest,
        cancellation: CancellationToken,
    ) -> ModelEventStream<'_> {
        stream::unfold(0_u8, move |step| {
            let cancellation = cancellation.clone();
            async move {
                match step {
                    0 => Some((
                        Ok(ModelEvent::ProviderRequest {
                            payload: json!({ "dispatched": true }),
                        }),
                        1,
                    )),
                    1 => {
                        cancellation.cancelled().await;
                        Some((
                            Ok(ModelEvent::Completed {
                                response: ModelResponse {
                                    content: vec![AssistantContent::text("definite")],
                                    stop_reason: StopReason::Stop,
                                    usage: None,
                                    metadata: AssistantMetadata::default(),
                                },
                            }),
                            2,
                        ))
                    }
                    _ => None,
                }
            }
        })
        .boxed()
    }
}

struct HangAfterDispatchModel;

impl Model for HangAfterDispatchModel {
    fn stream(
        &self,
        _request: ModelRequest,
        cancellation: CancellationToken,
    ) -> ModelEventStream<'_> {
        stream::unfold(false, move |dispatched| {
            let cancellation = cancellation.clone();
            async move {
                if dispatched {
                    cancellation.cancelled().await;
                    None
                } else {
                    Some((
                        Ok(ModelEvent::ProviderRequest {
                            payload: json!({ "dispatched": true }),
                        }),
                        true,
                    ))
                }
            }
        })
        .boxed()
    }
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
