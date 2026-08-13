use std::sync::atomic::{AtomicUsize, Ordering};

use futures_util::{StreamExt, stream};
use renoa_agent::{Model, ModelEventStream, ModelRequest, SamplingError, sample_model};
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
