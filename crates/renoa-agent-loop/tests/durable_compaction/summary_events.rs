//! A compaction summary is not an assistant message, so no surface may be
//! told it is one. The loop gives the summary request its own effect binding
//! and that binding never emits the assistant-message events.

use std::{
    collections::VecDeque,
    sync::{Arc, Mutex},
};

use futures_util::{StreamExt, stream};
use renoa_agent::{
    AgentEvent, AgentEventSink, AssistantDelta, BoxFuture, Model, ModelEvent, ModelEventStream,
    ModelRequest, ModelResponse, TokenUsage,
};
use renoa_agent_loop::{
    AgentLoopConfig, ContextBinding, ModelBinding, build_runtime_with_events, recorded_token_usage,
};
use renoa_kernel::EffectRecovery;
use tempfile::tempdir;
use tokio_util::sync::CancellationToken;

use crate::compaction_support::{
    SUMMARY, ThresholdSizer, compacting_strategy, create_session, nz32, submit_and_drive,
    text_response,
};

/// Streams every response's text as one delta, as a real provider does.
struct StreamingModel {
    responses: Mutex<VecDeque<ModelResponse>>,
}

impl Model for StreamingModel {
    fn stream(
        &self,
        _request: ModelRequest,
        _cancellation: CancellationToken,
    ) -> ModelEventStream<'_> {
        let response = self
            .responses
            .lock()
            .expect("response lock")
            .pop_front()
            .expect("streaming model ran out of responses");
        let mut events = Vec::new();
        for (index, block) in response.content.iter().enumerate() {
            if let renoa_agent::AssistantContent::Text { text, .. } = block {
                events.push(Ok(ModelEvent::ContentDelta {
                    content_index: index,
                    delta: AssistantDelta::Text { text: text.clone() },
                }));
            }
        }
        events.push(Ok(ModelEvent::Completed { response }));
        stream::iter(events).boxed()
    }
}

#[derive(Default)]
struct RecordingSink {
    events: Mutex<Vec<AgentEvent>>,
}

impl AgentEventSink for RecordingSink {
    fn emit(&self, event: AgentEvent) -> BoxFuture<'_, ()> {
        self.events.lock().expect("event lock").push(event);
        Box::pin(async {})
    }
}

fn with_usage(mut response: ModelResponse, input: u64) -> ModelResponse {
    response.usage = Some(TokenUsage {
        input,
        output: 1,
        cache_read: 0,
        cache_write: 0,
    });
    response
}

/// What one model request emitted: whether it was the summary request, how
/// many request-level and assistant-message events it produced, and how many
/// content blocks its end reported.
struct Invocation {
    is_summary: bool,
    request_chunks: usize,
    message_events: usize,
    response_blocks: usize,
}

fn invocations(events: &[AgentEvent]) -> Vec<Invocation> {
    let mut found: Vec<Invocation> = Vec::new();
    for event in events {
        match event {
            AgentEvent::ModelRequestStart { request, .. } => found.push(Invocation {
                is_summary: request
                    .system_prompt
                    .starts_with("You create durable context checkpoints"),
                request_chunks: 0,
                message_events: 0,
                response_blocks: 0,
            }),
            AgentEvent::ModelRequestChunk { .. } => {
                found
                    .last_mut()
                    .expect("chunk after a request")
                    .request_chunks += 1;
            }
            AgentEvent::MessageStart { .. }
            | AgentEvent::MessageUpdate { .. }
            | AgentEvent::MessageAbort => {
                found
                    .last_mut()
                    .expect("message after a request")
                    .message_events += 1;
            }
            AgentEvent::ModelRequestEnd { response, .. } => {
                found
                    .last_mut()
                    .expect("end after a request")
                    .response_blocks += response.content.len();
            }
            _ => {}
        }
    }
    found
}

#[tokio::test]
async fn a_summary_never_emits_assistant_message_events_and_a_reply_still_does() {
    let directory = tempdir().expect("temporary directory");
    let kernel =
        renoa_kernel::Kernel::open(directory.path().join("kernel.sqlite3")).expect("open kernel");
    let session_id = create_session(&kernel);
    let model = Arc::new(StreamingModel {
        responses: Mutex::new(VecDeque::from([
            with_usage(text_response("First answer."), 5),
            with_usage(text_response(SUMMARY), 7),
            with_usage(text_response("Second answer."), 11),
        ])),
    });
    let sink = Arc::new(RecordingSink::default());
    let strategy = compacting_strategy(Arc::new(ThresholdSizer), nz32(2));
    let runtime = build_runtime_with_events(
        AgentLoopConfig::new("Test summary events.", nz32(4), nz32(2)),
        ContextBinding::new("test-summary-events-v1", Arc::new(strategy)),
        ModelBinding::new("model-v1", model, EffectRecovery::SafeToReplay),
        Vec::new(),
        sink.clone(),
    )
    .expect("build runtime");

    submit_and_drive(&kernel, session_id, &runtime, "First question.").await;
    submit_and_drive(&kernel, session_id, &runtime, "Second question.").await;

    let events = sink.events.lock().expect("event lock");
    let found = invocations(&events);
    assert_eq!(found.len(), 3, "reply, summary, reply");
    assert!(!found[0].is_summary && !found[2].is_summary);
    assert!(found[1].is_summary);
    assert!(
        found[1].request_chunks > 0,
        "the summary's request-level events still reach observers"
    );
    assert_eq!(
        found[1].message_events, 0,
        "a summary is not an assistant message"
    );
    assert_eq!(
        found[1].response_blocks, 0,
        "a summary's end carries no content an observer could show"
    );
    assert!(
        found[0].response_blocks > 0 && found[2].response_blocks > 0,
        "a reply's end still carries its content"
    );
    assert!(
        found[0].message_events >= 2 && found[2].message_events >= 2,
        "a reply still starts and updates an assistant message"
    );
    drop(events);

    let usage = recorded_token_usage(&kernel.inspect(session_id).expect("inspect session"))
        .expect("every response reported usage");
    assert_eq!(
        usage.input,
        5 + 7 + 11,
        "the summary attempt stays in the session's usage"
    );

    let bindings = &runtime.manifest().effect_bindings;
    assert!(bindings.contains_key("renoa.agent.compaction"));
    assert_eq!(
        bindings.get("renoa.agent.compaction"),
        bindings.get("renoa.agent.model"),
        "the summary binding runs the same model revision as a reply"
    );
}
