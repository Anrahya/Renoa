use std::sync::{
    Arc, Mutex,
    atomic::{AtomicUsize, Ordering},
};

use futures_util::{StreamExt, stream};
use renoa_agent::{
    Agent, AgentError, AgentEvent, AgentEventSink, AssistantContent, AssistantDelta, BoxFuture,
    ContentBlock, Message, MessageRole, Model, ModelError, ModelEvent, ModelEventStream,
    ModelRequest, ModelResponse, StopReason, TokenUsage, Tool, ToolCall, ToolError, ToolOutput,
    ToolResult, ToolSpec, ToolUpdates,
};
use serde_json::json;
use tokio_util::sync::CancellationToken;

#[tokio::test]
async fn structured_stream_drives_its_tool_call_through_completion() {
    let model = Arc::new(StreamingModel::default());
    let events = Arc::new(RecordingSink::default());
    let mut agent = Agent::new(model, "You are concise.")
        .with_tools(vec![Arc::new(ReadTool::new())])
        .expect("tool name must be unique")
        .with_event_sink(events.clone());

    let result = agent
        .prompt("Hello")
        .await
        .expect("streamed prompt must complete");
    assert_eq!(result.output, "Hello back.");

    let user = Message::user_text("Hello");
    let call = streamed_tool_call();
    let first_assistant = Message::Assistant {
        content: vec![
            AssistantContent::reasoning("Need the file.", None, false),
            AssistantContent::text("Inspecting."),
            AssistantContent::tool_call(call.clone()),
        ],
        stop_reason: StopReason::ToolUse,
        usage: Some(TokenUsage {
            input: 4,
            output: 2,
            cache_read: 3,
            cache_write: 1,
        }),
        metadata: renoa_agent::AssistantMetadata::default(),
    };
    let tool_result = ToolResult {
        call_id: call.id.clone(),
        name: call.name.clone(),
        content: vec![ContentBlock::text("file contents")],
        details: None,
        is_error: false,
    };
    let final_assistant = Message::Assistant {
        content: vec![AssistantContent::text("Hello back.")],
        stop_reason: StopReason::Stop,
        usage: None,
        metadata: renoa_agent::AssistantMetadata::default(),
    };
    assert_eq!(
        agent.state().messages(),
        &[
            user.clone(),
            first_assistant.clone(),
            Message::Tool {
                result: tool_result.clone(),
            },
            final_assistant.clone(),
        ],
        "transient deltas must not enter conversation state",
    );
    let expected = [
        expected_user_events(user),
        expected_tool_turn_events(first_assistant, call, tool_result),
        expected_final_turn_events(final_assistant),
    ]
    .concat();
    assert_eq!(events.events(), expected);
}

fn expected_user_events(user: Message) -> Vec<AgentEvent> {
    vec![
        AgentEvent::AgentStart,
        AgentEvent::TurnStart,
        AgentEvent::MessageStart {
            role: MessageRole::User,
        },
        AgentEvent::MessageEnd { message: user },
    ]
}

fn expected_tool_turn_events(
    assistant: Message,
    call: ToolCall,
    result: ToolResult,
) -> Vec<AgentEvent> {
    vec![
        AgentEvent::MessageStart {
            role: MessageRole::Assistant,
        },
        AgentEvent::MessageUpdate {
            content_index: 0,
            delta: AssistantDelta::Reasoning {
                text: "Need the file.".to_owned(),
            },
        },
        AgentEvent::MessageUpdate {
            content_index: 1,
            delta: AssistantDelta::Text {
                text: "Inspecting.".to_owned(),
            },
        },
        AgentEvent::MessageUpdate {
            content_index: 2,
            delta: AssistantDelta::ToolCallStart {
                id: "call-1".to_owned(),
                name: "read".to_owned(),
            },
        },
        AgentEvent::MessageUpdate {
            content_index: 2,
            delta: AssistantDelta::ToolCallArguments {
                json_delta: r#"{"path":"src/lib.rs"}"#.to_owned(),
            },
        },
        AgentEvent::MessageEnd { message: assistant },
        AgentEvent::ToolExecutionStart { call: call.clone() },
        AgentEvent::ToolExecutionEnd {
            call,
            result: result.clone(),
        },
        AgentEvent::MessageStart {
            role: MessageRole::Tool,
        },
        AgentEvent::MessageEnd {
            message: Message::Tool { result },
        },
        AgentEvent::TurnEnd,
    ]
}

fn expected_final_turn_events(assistant: Message) -> Vec<AgentEvent> {
    vec![
        AgentEvent::TurnStart,
        AgentEvent::MessageStart {
            role: MessageRole::Assistant,
        },
        AgentEvent::MessageUpdate {
            content_index: 0,
            delta: AssistantDelta::Text {
                text: "Hello back.".to_owned(),
            },
        },
        AgentEvent::MessageEnd { message: assistant },
        AgentEvent::TurnEnd,
        AgentEvent::AgentEnd,
    ]
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
                delta: AssistantDelta::Text {
                    text: "Partial".to_owned(),
                },
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
                delta: AssistantDelta::Text {
                    text: "Partial".to_owned(),
                },
            },
            AgentEvent::MessageAbort,
            AgentEvent::TurnEnd,
            AgentEvent::AgentEnd,
        ]
    );
}

#[derive(Default)]
struct StreamingModel {
    turn: AtomicUsize,
}

impl Model for StreamingModel {
    fn stream(
        &self,
        _request: ModelRequest,
        _cancellation: CancellationToken,
    ) -> ModelEventStream<'_> {
        match self.turn.fetch_add(1, Ordering::Relaxed) {
            0 => stream::iter([
                Ok(ModelEvent::ContentDelta {
                    content_index: 0,
                    delta: AssistantDelta::Reasoning {
                        text: "Need the file.".to_owned(),
                    },
                }),
                Ok(ModelEvent::ContentDelta {
                    content_index: 1,
                    delta: AssistantDelta::Text {
                        text: "Inspecting.".to_owned(),
                    },
                }),
                Ok(ModelEvent::ContentDelta {
                    content_index: 2,
                    delta: AssistantDelta::ToolCallStart {
                        id: "call-1".to_owned(),
                        name: "read".to_owned(),
                    },
                }),
                Ok(ModelEvent::ContentDelta {
                    content_index: 2,
                    delta: AssistantDelta::ToolCallArguments {
                        json_delta: r#"{"path":"src/lib.rs"}"#.to_owned(),
                    },
                }),
                Ok(ModelEvent::Completed {
                    response: ModelResponse {
                        content: vec![
                            AssistantContent::reasoning("Need the file.", None, false),
                            AssistantContent::text("Inspecting."),
                            AssistantContent::tool_call(streamed_tool_call()),
                        ],
                        stop_reason: StopReason::ToolUse,
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
            .boxed(),
            1 => stream::iter([
                Ok(ModelEvent::ContentDelta {
                    content_index: 0,
                    delta: AssistantDelta::Text {
                        text: "Hello back.".to_owned(),
                    },
                }),
                Ok(ModelEvent::Completed {
                    response: ModelResponse {
                        content: vec![AssistantContent::text("Hello back.")],
                        stop_reason: StopReason::Stop,
                        usage: None,
                        metadata: renoa_agent::AssistantMetadata::default(),
                    },
                }),
            ])
            .boxed(),
            turn => panic!("unexpected model turn {turn}"),
        }
    }
}

struct ReadTool {
    spec: ToolSpec,
}

impl ReadTool {
    fn new() -> Self {
        Self {
            spec: ToolSpec {
                name: "read".to_owned(),
                description: "Read one file.".to_owned(),
                input_schema: json!({ "type": "object" }),
            },
        }
    }
}

impl Tool for ReadTool {
    fn spec(&self) -> &ToolSpec {
        &self.spec
    }

    fn execute(
        &self,
        call: ToolCall,
        _cancellation: CancellationToken,
        _updates: ToolUpdates,
    ) -> BoxFuture<'_, Result<ToolOutput, ToolError>> {
        Box::pin(async move {
            assert_eq!(call, streamed_tool_call());
            Ok(ToolOutput {
                content: vec![ContentBlock::text("file contents")],
                details: None,
            })
        })
    }
}

fn streamed_tool_call() -> ToolCall {
    ToolCall {
        id: "call-1".to_owned(),
        name: "read".to_owned(),
        arguments: json!({ "path": "src/lib.rs" }),
        thought_signature: None,
        namespace: None,
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
            Ok(ModelEvent::ContentDelta {
                content_index: 0,
                delta: AssistantDelta::Text {
                    text: "Partial".to_owned(),
                },
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
            Ok(ModelEvent::ContentDelta {
                content_index: 0,
                delta: AssistantDelta::Text {
                    text: "Partial".to_owned(),
                },
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
