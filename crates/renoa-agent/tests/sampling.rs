use std::sync::atomic::{AtomicUsize, Ordering};

use futures_util::{StreamExt, stream};
use renoa_agent::{
    AssistantDelta, Model, ModelError, ModelErrorKind, ModelEvent, ModelEventStream, ModelRequest,
    SamplingError, sample_model,
};
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
