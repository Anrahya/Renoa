use std::sync::{Arc, Mutex};

use futures_util::{StreamExt, stream};
use renoa_agent::{
    Agent, AgentEvent, AgentEventSink, AssistantContent, AssistantMetadata, BoxFuture,
    ContentBlock, Message, Model, ModelEvent, ModelEventStream, ModelRequest, ModelResponse,
    StopReason, Tool, ToolCall, ToolError, ToolOutput, ToolResult, ToolSpec, ToolUpdates,
};
use serde_json::json;
use tokio_util::sync::CancellationToken;

#[tokio::test]
async fn tool_progress_is_transient_and_final_details_enter_history() {
    let call = ToolCall {
        id: "call-1".to_owned(),
        name: "edit".to_owned(),
        arguments: json!({ "path": "src/lib.rs" }),
        thought_signature: None,
        namespace: None,
    };
    let model = Arc::new(ScriptedModel::new(vec![
        ModelResponse {
            content: vec![AssistantContent::tool_call(call.clone())],
            stop_reason: StopReason::ToolUse,
            usage: None,
            metadata: AssistantMetadata::default(),
        },
        ModelResponse {
            content: vec![AssistantContent::text("Done.")],
            stop_reason: StopReason::Stop,
            usage: None,
            metadata: AssistantMetadata::default(),
        },
    ]));
    let events = Arc::new(RecordingSink::default());
    let mut agent = Agent::new(model, "Edit carefully.")
        .with_tools(vec![Arc::new(EditTool::new())])
        .expect("tool name must be unique")
        .with_event_sink(events.clone());

    agent
        .prompt("Make the edit")
        .await
        .expect("run must finish");

    let progress = ToolOutput {
        content: vec![ContentBlock::text("computing diff")],
        details: Some(json!({ "phase": "diff" })),
    };
    let result = ToolResult {
        call_id: call.id.clone(),
        name: call.name.clone(),
        content: vec![
            ContentBlock::text("edited src/lib.rs"),
            ContentBlock::image("diff-preview", "image/png"),
        ],
        details: Some(json!({ "patch": "@@ -1 +1 @@" })),
        is_error: false,
    };
    let tool_events = events
        .events()
        .into_iter()
        .filter(|event| {
            matches!(
                event,
                AgentEvent::ToolExecutionStart { .. }
                    | AgentEvent::ToolExecutionUpdate { .. }
                    | AgentEvent::ToolExecutionEnd { .. }
            )
        })
        .collect::<Vec<_>>();
    assert_eq!(
        tool_events,
        vec![
            AgentEvent::ToolExecutionStart { call: call.clone() },
            AgentEvent::ToolExecutionUpdate {
                call: call.clone(),
                update: progress,
            },
            AgentEvent::ToolExecutionEnd {
                call,
                result: result.clone(),
            },
        ]
    );
    assert!(agent.state().messages().contains(&Message::Tool { result }));
    assert_eq!(
        agent
            .state()
            .messages()
            .iter()
            .filter(|message| matches!(message, Message::Tool { .. }))
            .count(),
        1,
        "progress must not become model history",
    );
}

struct EditTool {
    spec: ToolSpec,
}

impl EditTool {
    fn new() -> Self {
        Self {
            spec: ToolSpec {
                name: "edit".to_owned(),
                description: "Edit one file.".to_owned(),
                input_schema: json!({ "type": "object" }),
            },
        }
    }
}

impl Tool for EditTool {
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
                    content: vec![ContentBlock::text("computing diff")],
                    details: Some(json!({ "phase": "diff" })),
                })
                .await;
            Ok(ToolOutput {
                content: vec![
                    ContentBlock::text("edited src/lib.rs"),
                    ContentBlock::image("diff-preview", "image/png"),
                ],
                details: Some(json!({ "patch": "@@ -1 +1 @@" })),
            })
        })
    }
}

struct ScriptedModel {
    responses: Mutex<std::collections::VecDeque<ModelResponse>>,
}

impl ScriptedModel {
    fn new(responses: Vec<ModelResponse>) -> Self {
        Self {
            responses: Mutex::new(responses.into()),
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
            .expect("model response lock must not be poisoned")
            .pop_front()
            .expect("scripted response must exist");
        stream::once(async { Ok(ModelEvent::Completed { response }) }).boxed()
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
            .expect("event lock must not be poisoned")
            .clone()
    }
}

impl AgentEventSink for RecordingSink {
    fn emit(&self, event: AgentEvent) -> BoxFuture<'_, ()> {
        Box::pin(async move {
            self.events
                .lock()
                .expect("event lock must not be poisoned")
                .push(event);
        })
    }
}
