use std::sync::{Arc, Mutex};

use futures_util::{StreamExt, stream};
use renoa_agent::{
    Agent, AgentConfigError, AssistantContent, BoxFuture, ContentBlock, Model, ModelEvent,
    ModelEventStream, ModelRequest, ModelResponse, StopReason, Tool, ToolCall, ToolError,
    ToolOutput, ToolSpec, ToolUpdates,
};
use serde_json::json;
use tokio_util::sync::CancellationToken;

#[tokio::test]
async fn model_instructions_and_tools_can_change_safely_between_runs() {
    let first_model = Arc::new(RecordingModel::new(["first"]));
    let second_model = Arc::new(RecordingModel::new(["second", "third"]));
    let mut agent = Agent::new(first_model, "first instructions")
        .with_tools(vec![Arc::new(FixtureTool::new("alpha"))])
        .expect("initial tool is unique");
    agent.prompt("one").await.expect("first run");

    agent.set_model(second_model.clone());
    agent.set_system_prompt("second instructions");
    agent
        .set_tools(vec![Arc::new(FixtureTool::new("beta"))])
        .expect("replacement tool is unique");
    agent.prompt("two").await.expect("second run");

    let duplicate = agent.set_tools(vec![
        Arc::new(FixtureTool::new("duplicate")),
        Arc::new(FixtureTool::new("duplicate")),
    ]);
    assert!(matches!(
        duplicate,
        Err(AgentConfigError::DuplicateToolName(name)) if name == "duplicate"
    ));
    agent.prompt("three").await.expect("third run");

    let requests = second_model.requests();
    assert_eq!(requests[0].system_prompt, "second instructions");
    assert_eq!(requests[0].tools, vec![tool_spec("beta")]);
    assert_eq!(requests[1].tools, vec![tool_spec("beta")]);
}

struct FixtureTool {
    spec: ToolSpec,
}

impl FixtureTool {
    fn new(name: &str) -> Self {
        Self {
            spec: tool_spec(name),
        }
    }
}

impl Tool for FixtureTool {
    fn spec(&self) -> &ToolSpec {
        &self.spec
    }

    fn execute(
        &self,
        _call: ToolCall,
        _cancellation: CancellationToken,
        _updates: ToolUpdates,
    ) -> BoxFuture<'_, Result<ToolOutput, ToolError>> {
        Box::pin(async {
            Ok(ToolOutput {
                content: vec![ContentBlock::text("unused")],
                details: None,
            })
        })
    }
}

struct RecordingModel {
    responses: Mutex<std::collections::VecDeque<String>>,
    requests: Mutex<Vec<ModelRequest>>,
}

impl RecordingModel {
    fn new<const N: usize>(responses: [&str; N]) -> Self {
        Self {
            responses: Mutex::new(responses.into_iter().map(str::to_owned).collect()),
            requests: Mutex::new(Vec::new()),
        }
    }

    fn requests(&self) -> Vec<ModelRequest> {
        self.requests.lock().expect("request lock").clone()
    }
}

impl Model for RecordingModel {
    fn stream(
        &self,
        request: ModelRequest,
        _cancellation: CancellationToken,
    ) -> ModelEventStream<'_> {
        self.requests.lock().expect("request lock").push(request);
        let text = self
            .responses
            .lock()
            .expect("response lock")
            .pop_front()
            .expect("scripted response");
        stream::once(async move {
            Ok(ModelEvent::Completed {
                response: ModelResponse {
                    content: vec![AssistantContent::text(text)],
                    stop_reason: StopReason::Stop,
                    usage: None,
                    metadata: renoa_agent::AssistantMetadata::default(),
                },
            })
        })
        .boxed()
    }
}

fn tool_spec(name: &str) -> ToolSpec {
    ToolSpec {
        name: name.to_owned(),
        description: "Fixture tool.".to_owned(),
        input_schema: json!({ "type": "object" }),
    }
}
