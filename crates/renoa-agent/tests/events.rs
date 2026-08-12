use std::sync::{Arc, Mutex};

use futures_util::{StreamExt, stream};
use renoa_agent::{
    Agent, AgentError, AgentEvent, AgentEventSink, AssistantContent, BoxFuture, Message,
    MessageRole, Model, ModelError, ModelEvent, ModelEventStream, ModelRequest, ModelResponse,
    StopReason, TokenUsage,
};
use tokio_util::sync::CancellationToken;

#[tokio::test]
async fn streamed_text_preserves_content_block_identity() {
    let model = Arc::new(StreamingModel);
    let events = Arc::new(RecordingSink::default());
    let mut agent = Agent::new(model, "You are concise.").with_event_sink(events.clone());

    let result = agent
        .prompt("Hello")
        .await
        .expect("streamed prompt must complete");
    assert_eq!(result.output, "Hello back.");

    let user = Message::user_text("Hello");
    let assistant = Message::Assistant {
        content: vec![
            AssistantContent::text("Hello "),
            AssistantContent::text("back."),
        ],
        stop_reason: StopReason::Stop,
        usage: Some(TokenUsage {
            input: 4,
            output: 2,
            cache_read: 3,
            cache_write: 1,
        }),
        metadata: renoa_agent::AssistantMetadata::default(),
    };
    assert_eq!(
        events.events(),
        vec![
            AgentEvent::AgentStart,
            AgentEvent::TurnStart,
            AgentEvent::MessageStart {
                role: MessageRole::User,
            },
            AgentEvent::MessageEnd { message: user },
            AgentEvent::MessageStart {
                role: MessageRole::Assistant,
            },
            AgentEvent::MessageUpdate {
                content_index: 0,
                text_delta: "Hello ".to_owned(),
            },
            AgentEvent::MessageUpdate {
                content_index: 1,
                text_delta: "back.".to_owned(),
            },
            AgentEvent::MessageEnd { message: assistant },
            AgentEvent::TurnEnd,
            AgentEvent::AgentEnd,
        ]
    );
}

#[tokio::test]
async fn failed_stream_aborts_partial_text_without_persisting_it() {
    let events = Arc::new(RecordingSink::default());
    let mut agent = Agent::new(Arc::new(FailingStreamingModel), "You are concise.")
        .with_event_sink(events.clone());

    let error = agent
        .prompt("Hello")
        .await
        .expect_err("failed stream must fail the prompt");

    assert!(matches!(error, AgentError::Model(_)));
    let user = Message::user_text("Hello");
    assert_eq!(agent.state().messages(), std::slice::from_ref(&user));
    assert_eq!(
        events.events(),
        vec![
            AgentEvent::AgentStart,
            AgentEvent::TurnStart,
            AgentEvent::MessageStart {
                role: MessageRole::User,
            },
            AgentEvent::MessageEnd { message: user },
            AgentEvent::MessageStart {
                role: MessageRole::Assistant,
            },
            AgentEvent::MessageUpdate {
                content_index: 0,
                text_delta: "Partial".to_owned(),
            },
            AgentEvent::MessageAbort,
            AgentEvent::TurnEnd,
            AgentEvent::AgentEnd,
        ]
    );
}

#[tokio::test]
async fn incomplete_stream_aborts_partial_text_without_persisting_it() {
    let events = Arc::new(RecordingSink::default());
    let mut agent = Agent::new(Arc::new(IncompleteStreamingModel), "You are concise.")
        .with_event_sink(events.clone());

    let error = agent
        .prompt("Hello")
        .await
        .expect_err("a stream without completion must fail the prompt");

    assert!(matches!(error, AgentError::IncompleteModelStream));
    let user = Message::user_text("Hello");
    assert_eq!(agent.state().messages(), std::slice::from_ref(&user));
    assert_eq!(
        events.events(),
        vec![
            AgentEvent::AgentStart,
            AgentEvent::TurnStart,
            AgentEvent::MessageStart {
                role: MessageRole::User,
            },
            AgentEvent::MessageEnd { message: user },
            AgentEvent::MessageStart {
                role: MessageRole::Assistant,
            },
            AgentEvent::MessageUpdate {
                content_index: 0,
                text_delta: "Partial".to_owned(),
            },
            AgentEvent::MessageAbort,
            AgentEvent::TurnEnd,
            AgentEvent::AgentEnd,
        ]
    );
}

struct StreamingModel;

impl Model for StreamingModel {
    fn stream(
        &self,
        _request: ModelRequest,
        _cancellation: CancellationToken,
    ) -> ModelEventStream<'_> {
        stream::iter([
            Ok(ModelEvent::TextDelta {
                content_index: 0,
                text: "Hello ".to_owned(),
            }),
            Ok(ModelEvent::TextDelta {
                content_index: 1,
                text: "back.".to_owned(),
            }),
            Ok(ModelEvent::Completed {
                response: ModelResponse {
                    content: vec![
                        AssistantContent::text("Hello "),
                        AssistantContent::text("back."),
                    ],
                    stop_reason: StopReason::Stop,
                    usage: Some(TokenUsage {
                        input: 4,
                        output: 2,
                        cache_read: 3,
                        cache_write: 1,
                    }),
                    metadata: renoa_agent::AssistantMetadata::default(),
                },
            }),
        ])
        .boxed()
    }
}

struct FailingStreamingModel;

impl Model for FailingStreamingModel {
    fn stream(
        &self,
        _request: ModelRequest,
        _cancellation: CancellationToken,
    ) -> ModelEventStream<'_> {
        stream::iter([
            Ok(ModelEvent::TextDelta {
                content_index: 0,
                text: "Partial".to_owned(),
            }),
            Err(ModelError::new("provider disconnected")),
        ])
        .boxed()
    }
}

struct IncompleteStreamingModel;

impl Model for IncompleteStreamingModel {
    fn stream(
        &self,
        _request: ModelRequest,
        _cancellation: CancellationToken,
    ) -> ModelEventStream<'_> {
        stream::once(async {
            Ok(ModelEvent::TextDelta {
                content_index: 0,
                text: "Partial".to_owned(),
            })
        })
        .boxed()
    }
}

#[derive(Default)]
struct RecordingSink {
    events: Mutex<Vec<AgentEvent>>,
}

impl RecordingSink {
    fn events(&self) -> Vec<AgentEvent> {
        self.events
            .lock()
            .expect("event sink lock must not be poisoned")
            .clone()
    }
}

impl AgentEventSink for RecordingSink {
    fn emit(&self, event: AgentEvent) -> BoxFuture<'_, ()> {
        Box::pin(async move {
            self.events
                .lock()
                .expect("event sink lock must not be poisoned")
                .push(event);
        })
    }
}
