use std::sync::{Arc, Mutex};

use futures_util::stream;
use renoa_agent::{
    Agent, AgentConfig, AgentError, AssistantContent, AssistantMetadata, BoxFuture, Message, Model,
    ModelEvent, ModelEventStream, ModelRequest, ModelResponse, StopReason, Tool, ToolCall,
    ToolError, ToolExecutionMode, ToolOutput, ToolSpec, ToolUpdates, invoke_tool,
};
use tokio_util::sync::CancellationToken;

#[tokio::test]
async fn uncertain_tool_outcome_is_not_converted_to_a_model_visible_failure() {
    let call = ToolCall {
        id: "publish-1".to_owned(),
        name: "publish".to_owned(),
        arguments: serde_json::json!({"document": "release-notes"}),
        thought_signature: None,
        namespace: None,
    };

    let tool = UncertainTool::new("publish");
    let error = invoke_tool(Some(&tool), call.clone(), CancellationToken::new(), None)
        .await
        .expect_err("an uncertain external action must not become a settled tool result");

    assert_eq!(error.call_id(), call.id);
    assert_eq!(error.tool_name(), call.name);
    assert_eq!(error.message(), "connection closed after dispatch");
}

#[tokio::test]
async fn parallel_agent_execution_preserves_every_uncertain_tool_outcome() {
    let calls = [
        tool_call("publish-1", "publish"),
        tool_call("notify-1", "notify"),
    ];
    let model = Arc::new(SingleResponseModel::new(ModelResponse {
        content: calls.into_iter().map(AssistantContent::tool_call).collect(),
        stop_reason: StopReason::ToolUse,
        usage: None,
        metadata: AssistantMetadata::default(),
    }));
    let mut agent = Agent::new(model, "Run independent tools concurrently.")
        .with_tools(vec![
            Arc::new(UncertainTool::new("publish")),
            Arc::new(UncertainTool::new("notify")),
        ])
        .expect("unique tool names must be accepted");
    agent
        .set_config(AgentConfig {
            tool_execution: ToolExecutionMode::Parallel,
            ..AgentConfig::default()
        })
        .expect("empty input queues must accept the configuration");

    let error = agent
        .prompt("Publish and notify")
        .await
        .expect_err("uncertain parallel calls must fail the run honestly");
    let AgentError::ToolOutcomesUnknown { outcomes } = error else {
        panic!("expected typed uncertain tool outcomes");
    };
    let mut identities = outcomes
        .iter()
        .map(|outcome| (outcome.call_id(), outcome.tool_name()))
        .collect::<Vec<_>>();
    identities.sort_unstable();
    assert_eq!(
        identities,
        vec![("notify-1", "notify"), ("publish-1", "publish")]
    );
    assert_eq!(
        agent.state().messages(),
        &[
            Message::user_text("Publish and notify"),
            Message::Assistant {
                content: vec![
                    AssistantContent::tool_call(tool_call("publish-1", "publish")),
                    AssistantContent::tool_call(tool_call("notify-1", "notify")),
                ],
                stop_reason: StopReason::ToolUse,
                usage: None,
                metadata: AssistantMetadata::default(),
            },
        ]
    );
}

#[tokio::test]
async fn uncertain_outcome_blocks_restored_agent_until_explicit_reset() {
    let calls = [
        tool_call("publish-1", "publish"),
        tool_call("notify-1", "notify"),
    ];
    let model = Arc::new(SingleResponseModel::new(ModelResponse {
        content: calls.into_iter().map(AssistantContent::tool_call).collect(),
        stop_reason: StopReason::ToolUse,
        usage: None,
        metadata: AssistantMetadata::default(),
    }));
    let mut agent = Agent::new(model, "Run tools in order.")
        .with_tools(vec![
            Arc::new(UncertainTool::new("publish")),
            Arc::new(UncertainTool::new("notify")),
        ])
        .expect("unique tool names must be accepted");

    let error = agent
        .prompt("Publish and notify")
        .await
        .expect_err("the first uncertain call must block the run");
    let AgentError::ToolOutcomesUnknown { outcomes } = error else {
        panic!("expected a typed uncertain tool outcome");
    };
    assert_eq!(outcomes.len(), 1);
    assert_eq!(agent.state().unresolved_tool_outcomes(), outcomes);

    let encoded = serde_json::to_string(agent.state()).expect("blocked state must serialize");
    let state = serde_json::from_str(&encoded).expect("blocked state must restore");
    let mut restored = Agent::from_state(Arc::new(MustNotRunModel), "Stay blocked.", state);
    let messages = restored.state().messages().to_vec();

    let resume_error = restored
        .resume()
        .await
        .expect_err("restored uncertainty must block resume");
    assert_unknown_outcomes(resume_error, &outcomes);
    let prompt_error = restored
        .prompt("Do not append this")
        .await
        .expect_err("restored uncertainty must block a new prompt");
    assert_unknown_outcomes(prompt_error, &outcomes);
    assert_eq!(restored.state().messages(), messages);

    restored.reset();
    assert!(restored.state().unresolved_tool_outcomes().is_empty());
    assert!(restored.state().messages().is_empty());
    assert!(matches!(
        restored.resume().await,
        Err(AgentError::NothingToResume)
    ));
}

fn assert_unknown_outcomes(error: AgentError, expected: &[renoa_agent::ToolOutcomeUnknown]) {
    let AgentError::ToolOutcomesUnknown { outcomes } = error else {
        panic!("expected restored uncertainty to remain typed");
    };
    assert_eq!(outcomes, expected);
}

struct UncertainTool {
    spec: ToolSpec,
}

impl UncertainTool {
    fn new(name: &str) -> Self {
        Self {
            spec: ToolSpec {
                name: name.to_owned(),
                description: "Perform an external action.".to_owned(),
                input_schema: serde_json::json!({"type": "object"}),
            },
        }
    }
}

impl Tool for UncertainTool {
    fn spec(&self) -> &ToolSpec {
        &self.spec
    }

    fn execute(
        &self,
        _call: ToolCall,
        _cancellation: CancellationToken,
        _updates: ToolUpdates,
    ) -> BoxFuture<'_, Result<ToolOutput, ToolError>> {
        Box::pin(std::future::ready(Err(ToolError::outcome_unknown(
            "connection closed after dispatch",
        ))))
    }
}

struct SingleResponseModel {
    response: Mutex<Option<ModelResponse>>,
}

impl SingleResponseModel {
    fn new(response: ModelResponse) -> Self {
        Self {
            response: Mutex::new(Some(response)),
        }
    }
}

impl Model for SingleResponseModel {
    fn stream(
        &self,
        _request: ModelRequest,
        _cancellation: CancellationToken,
    ) -> ModelEventStream<'_> {
        let response = self
            .response
            .lock()
            .expect("response lock")
            .take()
            .expect("single response");
        Box::pin(stream::once(std::future::ready(Ok(
            ModelEvent::Completed { response },
        ))))
    }
}

struct MustNotRunModel;

impl Model for MustNotRunModel {
    fn stream(
        &self,
        _request: ModelRequest,
        _cancellation: CancellationToken,
    ) -> ModelEventStream<'_> {
        panic!("a blocked agent must not invoke its model")
    }
}

fn tool_call(id: &str, name: &str) -> ToolCall {
    ToolCall {
        id: id.to_owned(),
        name: name.to_owned(),
        arguments: serde_json::json!({}),
        thought_signature: None,
        namespace: None,
    }
}
