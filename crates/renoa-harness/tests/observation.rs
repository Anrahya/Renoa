use std::{
    collections::VecDeque,
    num::NonZeroU32,
    sync::{Arc, Mutex},
};

use futures_util::{StreamExt as _, stream};
use renoa_agent::{
    AgentEvent, AgentEventSink, AssistantContent, AssistantDelta, AssistantMetadata, BoxFuture,
    ContentBlock, MessageRole, Model, ModelEvent, ModelEventStream, ModelRequest, ModelResponse,
    StopReason, Tool, ToolCall, ToolError, ToolOutput, ToolSpec, ToolUpdates,
};
use renoa_harness::{
    Harness, OperationRequest, RequestId, RuntimeProfile, SessionId, ToolBinding, ToolRecovery,
};
use tempfile::tempdir;
use tokio_util::sync::CancellationToken;

#[tokio::test]
async fn a_harness_driver_forwards_transient_model_deltas_in_order() {
    let directory = tempdir().expect("temporary directory");
    let harness = Harness::open(directory.path().join("harness.sqlite3")).expect("open harness");
    let session_id = SessionId::new();
    harness
        .create_standalone_session(session_id)
        .await
        .expect("create session");
    harness
        .admit_standalone(
            session_id,
            OperationRequest::new(RequestId::new(), vec![ContentBlock::text("Say hello")]),
        )
        .await
        .expect("admit prompt");
    let profile = RuntimeProfile::new(
        "observed-v1",
        Arc::new(StreamingModel),
        "Be concise.",
        NonZeroU32::new(1).expect("non-zero attempts"),
    );
    let sink = RecordingSink::default();

    harness
        .run_next_with_events(session_id, &profile, &sink)
        .await
        .expect("run observed operation");

    assert_eq!(
        sink.events(),
        vec![
            AgentEvent::MessageStart {
                role: MessageRole::Assistant,
            },
            AgentEvent::MessageUpdate {
                content_index: 0,
                delta: AssistantDelta::Text {
                    text: "Hello ".to_owned(),
                },
            },
            AgentEvent::MessageUpdate {
                content_index: 0,
                delta: AssistantDelta::Text {
                    text: "world".to_owned(),
                },
            },
        ]
    );
}

#[tokio::test]
async fn tool_events_wrap_the_real_effect_and_its_durable_settlement() {
    let directory = tempdir().expect("temporary directory");
    let harness = Harness::open(directory.path().join("harness.sqlite3")).expect("open harness");
    let session_id = SessionId::new();
    harness
        .create_standalone_session(session_id)
        .await
        .expect("create session");
    harness
        .admit_standalone(
            session_id,
            OperationRequest::new(RequestId::new(), vec![ContentBlock::text("Use the tool")]),
        )
        .await
        .expect("admit prompt");
    let call = ToolCall {
        id: "call-1".to_owned(),
        name: "probe".to_owned(),
        arguments: serde_json::json!({ "value": 1 }),
        thought_signature: None,
        namespace: None,
    };
    let model = Arc::new(ScriptedModel::new([
        ModelResponse {
            content: vec![AssistantContent::tool_call(call.clone())],
            stop_reason: StopReason::ToolUse,
            usage: None,
            metadata: AssistantMetadata::default(),
        },
        ModelResponse {
            content: vec![AssistantContent::text("Done")],
            stop_reason: StopReason::Stop,
            usage: None,
            metadata: AssistantMetadata::default(),
        },
    ]));
    let profile = RuntimeProfile::new(
        "observed-tool-v1",
        model,
        "Use tools.",
        NonZeroU32::new(2).expect("non-zero attempts"),
    )
    .with_tools(
        vec![ToolBinding::new(
            "probe-v1",
            Arc::new(ProgressTool::new()),
            ToolRecovery::SafeToReplay,
        )],
        NonZeroU32::new(1).expect("non-zero tool limit"),
    )
    .expect("valid tool profile");
    let sink = RecordingSink::default();

    harness
        .run_next_with_events(session_id, &profile, &sink)
        .await
        .expect("run observed tool operation");

    let progress = ToolOutput {
        content: vec![ContentBlock::text("working")],
        details: None,
    };
    let result = renoa_agent::ToolResult {
        call_id: call.id.clone(),
        name: call.name.clone(),
        content: vec![ContentBlock::text("finished")],
        details: Some(serde_json::json!({ "value": 1 })),
        is_error: false,
    };
    assert_eq!(
        sink.events(),
        vec![
            AgentEvent::ToolExecutionStart { call: call.clone() },
            AgentEvent::ToolExecutionUpdate {
                call: call.clone(),
                update: progress,
            },
            AgentEvent::ToolExecutionEnd { call, result },
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
            Ok(ModelEvent::ContentDelta {
                content_index: 0,
                delta: AssistantDelta::Text {
                    text: "Hello ".to_owned(),
                },
            }),
            Ok(ModelEvent::ContentDelta {
                content_index: 0,
                delta: AssistantDelta::Text {
                    text: "world".to_owned(),
                },
            }),
            Ok(ModelEvent::Completed {
                response: ModelResponse {
                    content: vec![AssistantContent::text("Hello world")],
                    stop_reason: StopReason::Stop,
                    usage: None,
                    metadata: AssistantMetadata::default(),
                },
            }),
        ])
        .boxed()
    }
}

struct ScriptedModel {
    responses: Mutex<VecDeque<ModelResponse>>,
}

impl ScriptedModel {
    fn new(responses: impl IntoIterator<Item = ModelResponse>) -> Self {
        Self {
            responses: Mutex::new(responses.into_iter().collect()),
        }
    }
}

impl Model for ScriptedModel {
    fn stream(
        &self,
        _request: ModelRequest,
        _cancellation: CancellationToken,
    ) -> ModelEventStream<'_> {
        let response = self
            .responses
            .lock()
            .expect("model lock")
            .pop_front()
            .expect("scripted response");
        stream::once(async move { Ok(ModelEvent::Completed { response }) }).boxed()
    }
}

struct ProgressTool {
    spec: ToolSpec,
}

impl ProgressTool {
    fn new() -> Self {
        Self {
            spec: ToolSpec {
                name: "probe".to_owned(),
                description: "Reports progress.".to_owned(),
                input_schema: serde_json::json!({ "type": "object" }),
            },
        }
    }
}

impl Tool for ProgressTool {
    fn spec(&self) -> &ToolSpec {
        &self.spec
    }

    fn execute(
        &self,
        _call: ToolCall,
        _cancellation: CancellationToken,
        updates: ToolUpdates,
    ) -> BoxFuture<'_, Result<ToolOutput, ToolError>> {
        Box::pin(async move {
            updates
                .emit(ToolOutput {
                    content: vec![ContentBlock::text("working")],
                    details: None,
                })
                .await;
            Ok(ToolOutput {
                content: vec![ContentBlock::text("finished")],
                details: Some(serde_json::json!({ "value": 1 })),
            })
        })
    }
}

#[derive(Default)]
struct RecordingSink {
    events: Mutex<Vec<AgentEvent>>,
}

impl RecordingSink {
    fn events(&self) -> Vec<AgentEvent> {
        self.events.lock().expect("event lock").clone()
    }
}

impl AgentEventSink for RecordingSink {
    fn emit(&self, event: AgentEvent) -> BoxFuture<'_, ()> {
        Box::pin(async move {
            self.events.lock().expect("event lock").push(event);
        })
    }
}
