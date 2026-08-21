use std::{sync::Arc, time::Duration};

use futures_util::{FutureExt, StreamExt, stream};
use renoa_agent::{
    Agent, AgentError, AgentEvent, AgentEventSink, AssistantContent, BoxFuture, Model, ModelEvent,
    ModelEventStream, ModelRequest, ModelResponse, StopReason,
};
use tokio::sync::{Notify, Semaphore};
use tokio_util::sync::CancellationToken;

#[tokio::test]
async fn handle_aborts_the_active_prompt_and_becomes_idle() {
    let model = Arc::new(PendingModel::default());
    let mut agent = Agent::new(model.clone(), "You are concise.");
    let handle = agent.handle();
    let mut prompt = Box::pin(agent.prompt("Wait"));

    tokio::select! {
        biased;
        result = &mut prompt => panic!("prompt settled before cancellation: {result:?}"),
        () = model.wait_until_started() => {}
    }

    assert!(handle.is_running());
    handle.abort();
    let error = tokio::time::timeout(Duration::from_secs(1), &mut prompt)
        .await
        .expect("aborted prompt must settle")
        .expect_err("aborted prompt must fail");
    assert!(matches!(error, AgentError::Cancelled));
    handle.wait_for_idle().await;
    assert!(!handle.is_running());
}

#[tokio::test]
async fn wait_for_idle_includes_agent_end_listener_settlement() {
    let model = Arc::new(CompletedModel);
    let events = Arc::new(BlockingAgentEndSink::new());
    let mut agent = Agent::new(model, "You are concise.").with_event_sink(events.clone());
    let handle = agent.handle();
    let mut prompt = Box::pin(agent.prompt("Finish"));

    tokio::select! {
        biased;
        result = &mut prompt => panic!("prompt settled before agent_end listener: {result:?}"),
        () = events.wait_until_agent_end() => {}
    }

    assert!(handle.is_running());
    let mut idle = Box::pin(handle.wait_for_idle());
    assert!(idle.as_mut().now_or_never().is_none());

    events.release_agent_end();
    prompt.await.expect("prompt must complete");
    idle.await;
    assert!(!handle.is_running());
}

#[derive(Default)]
struct PendingModel {
    started: Notify,
}

impl PendingModel {
    async fn wait_until_started(&self) {
        self.started.notified().await;
    }
}

impl Model for PendingModel {
    fn stream(
        &self,
        _request: ModelRequest,
        cancellation: CancellationToken,
    ) -> ModelEventStream<'_> {
        self.started.notify_one();
        stream::once(async move {
            cancellation.cancelled().await;
            Err(renoa_agent::ModelError::new("cancelled model stopped"))
        })
        .boxed()
    }
}

struct CompletedModel;

impl Model for CompletedModel {
    fn stream(
        &self,
        _request: ModelRequest,
        _cancellation: CancellationToken,
    ) -> ModelEventStream<'_> {
        stream::once(async {
            Ok(ModelEvent::Completed {
                response: ModelResponse {
                    content: vec![AssistantContent::text("Done.")],
                    stop_reason: StopReason::Stop,
                    usage: None,
                    metadata: renoa_agent::AssistantMetadata::default(),
                },
            })
        })
        .boxed()
    }
}

struct BlockingAgentEndSink {
    reached_agent_end: Semaphore,
    release_agent_end: Semaphore,
}

impl BlockingAgentEndSink {
    fn new() -> Self {
        Self {
            reached_agent_end: Semaphore::new(0),
            release_agent_end: Semaphore::new(0),
        }
    }

    async fn wait_until_agent_end(&self) {
        self.reached_agent_end
            .acquire()
            .await
            .expect("agent_end semaphore must remain open")
            .forget();
    }

    fn release_agent_end(&self) {
        self.release_agent_end.add_permits(1);
    }
}

impl AgentEventSink for BlockingAgentEndSink {
    fn emit(&self, event: AgentEvent) -> BoxFuture<'_, ()> {
        Box::pin(async move {
            if event == AgentEvent::AgentEnd {
                self.reached_agent_end.add_permits(1);
                self.release_agent_end
                    .acquire()
                    .await
                    .expect("agent_end release semaphore must remain open")
                    .forget();
            }
        })
    }
}
