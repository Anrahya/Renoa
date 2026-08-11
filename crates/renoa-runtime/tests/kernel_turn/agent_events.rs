use std::{
    fs,
    sync::{Arc, Mutex},
};

use crate::support::{ModelStep, ScriptedModel, TestCapabilityHost, test_agent, test_command};
use renoa_core::{
    BoxFuture, CapabilityCall, CapabilityOutcome, CommandInput, Message, ModelError, ModelEvent,
    ModelResponse,
};
use renoa_runtime::{Agent, AgentEvent, AgentEventSink, Engine, EngineConfig};
use renoa_store_sqlite::SqliteRunStore;
use serde_json::json;
use tempfile::tempdir;
use tokio_util::sync::CancellationToken;

#[tokio::test]
async fn event_sink_observes_a_complete_prompt_lifecycle() {
    let workspace = tempdir().expect("temporary workspace must be created");
    let model = Arc::new(ScriptedModel::new(vec![final_response("Done.")]));
    let engine = Engine::new(
        model,
        Arc::new(TestCapabilityHost::new(workspace.path())),
        Arc::new(
            SqliteRunStore::open(workspace.path().join("renoa.db")).expect("run store must open"),
        ),
        EngineConfig::default(),
    );
    let events = Arc::new(RecordingEventSink::default());
    let mut agent = Agent::new(engine, test_agent()).with_event_sink(events.clone());
    let mut command = test_command();
    command.input = CommandInput::Text {
        text: "Do the work.".to_owned(),
    };

    agent
        .prompt(command, CancellationToken::new())
        .await
        .expect("prompt must complete");

    let user = Message::User {
        text: "Do the work.".to_owned(),
    };
    let assistant = Message::Assistant {
        text: "Done.".to_owned(),
        capability_calls: Vec::new(),
    };
    assert_eq!(
        events.events(),
        vec![
            AgentEvent::AgentStart,
            AgentEvent::TurnStart,
            AgentEvent::MessageStart {
                message: user.clone(),
            },
            AgentEvent::MessageEnd { message: user },
            AgentEvent::MessageStart {
                message: assistant.clone(),
            },
            AgentEvent::MessageEnd { message: assistant },
            AgentEvent::TurnEnd,
            AgentEvent::AgentEnd,
        ]
    );
}

#[tokio::test]
async fn event_sink_observes_streamed_text_before_the_completed_message() {
    let workspace = tempdir().expect("temporary workspace must be created");
    let model = Arc::new(ScriptedModel::from_steps(vec![ModelStep::Events(vec![
        Ok(ModelEvent::TextDelta {
            text: "Hel".to_owned(),
        }),
        Ok(ModelEvent::TextDelta {
            text: "lo".to_owned(),
        }),
        Ok(ModelEvent::Completed {
            response: final_response("Hello"),
        }),
    ])]));
    let engine = Engine::new(
        model,
        Arc::new(TestCapabilityHost::new(workspace.path())),
        Arc::new(
            SqliteRunStore::open(workspace.path().join("renoa.db")).expect("run store must open"),
        ),
        EngineConfig::default(),
    );
    let events = Arc::new(RecordingEventSink::default());
    let mut agent = Agent::new(engine, test_agent()).with_event_sink(events.clone());

    agent
        .prompt(test_command(), CancellationToken::new())
        .await
        .expect("streamed prompt must complete");

    let partial = |text: &str| Message::Assistant {
        text: text.to_owned(),
        capability_calls: Vec::new(),
    };
    let expected = [
        AgentEvent::MessageStart {
            message: partial(""),
        },
        AgentEvent::MessageUpdate {
            text_delta: "Hel".to_owned(),
        },
        AgentEvent::MessageUpdate {
            text_delta: "lo".to_owned(),
        },
        AgentEvent::MessageEnd {
            message: partial("Hello"),
        },
    ];
    assert!(
        events
            .events()
            .windows(expected.len())
            .any(|events| events == expected)
    );
}

#[tokio::test]
async fn event_sink_aborts_partial_message_when_model_stream_fails() {
    let workspace = tempdir().expect("temporary workspace must be created");
    let model = Arc::new(ScriptedModel::from_steps(vec![ModelStep::Events(vec![
        Ok(ModelEvent::TextDelta {
            text: "Partial".to_owned(),
        }),
        Err(ModelError::new("provider disconnected")),
    ])]));
    let engine = Engine::new(
        model,
        Arc::new(TestCapabilityHost::new(workspace.path())),
        Arc::new(
            SqliteRunStore::open(workspace.path().join("renoa.db")).expect("run store must open"),
        ),
        EngineConfig::default(),
    );
    let events = Arc::new(RecordingEventSink::default());
    let mut agent = Agent::new(engine, test_agent()).with_event_sink(events.clone());

    let result = agent.prompt(test_command(), CancellationToken::new()).await;

    assert!(matches!(result, Err(renoa_runtime::EngineError::Model(_))));
    let expected = [
        AgentEvent::MessageUpdate {
            text_delta: "Partial".to_owned(),
        },
        AgentEvent::MessageAbort,
        AgentEvent::TurnEnd,
        AgentEvent::AgentEnd,
    ];
    assert!(
        events
            .events()
            .windows(expected.len())
            .any(|events| events == expected)
    );
    assert_eq!(
        serde_json::to_value(agent.state()).expect("agent state must serialize"),
        json!({
            "messages": [{
                "role": "user",
                "text": "Read hello.txt, add a second line, verify it, and report back."
            }]
        })
    );
}

#[tokio::test]
async fn event_sink_aborts_partial_message_when_streaming_is_cancelled() {
    let workspace = tempdir().expect("temporary workspace must be created");
    let model = Arc::new(ScriptedModel::from_steps(vec![
        ModelStep::EventsThenPending(vec![Ok(ModelEvent::TextDelta {
            text: "Partial".to_owned(),
        })]),
    ]));
    let engine = Engine::new(
        model,
        Arc::new(TestCapabilityHost::new(workspace.path())),
        Arc::new(
            SqliteRunStore::open(workspace.path().join("renoa.db")).expect("run store must open"),
        ),
        EngineConfig::default(),
    );
    let cancellation = CancellationToken::new();
    let events = Arc::new(RecordingEventSink::cancelling(cancellation.clone()));
    let mut agent = Agent::new(engine, test_agent()).with_event_sink(events.clone());

    let result = agent.prompt(test_command(), cancellation).await;

    assert!(matches!(result, Err(renoa_runtime::EngineError::Cancelled)));
    let expected = [
        AgentEvent::MessageUpdate {
            text_delta: "Partial".to_owned(),
        },
        AgentEvent::MessageAbort,
        AgentEvent::TurnEnd,
        AgentEvent::AgentEnd,
    ];
    assert!(
        events
            .events()
            .windows(expected.len())
            .any(|events| events == expected)
    );
}

#[tokio::test]
async fn event_sink_observes_tool_execution_before_its_result_message() {
    let workspace = tempdir().expect("temporary workspace must be created");
    fs::write(workspace.path().join("tool.txt"), "tool output")
        .expect("tool fixture must be written");
    let call = CapabilityCall {
        call_id: "read-tool".to_owned(),
        name: "read_file".to_owned(),
        arguments: json!({ "path": "tool.txt" }),
    };
    let outcome = CapabilityOutcome {
        model_view: json!({ "content": "tool output" }),
        is_error: false,
    };
    let model = Arc::new(ScriptedModel::new(vec![
        ModelResponse {
            text: String::new(),
            capability_calls: vec![call.clone()],
            truncated: false,
        },
        final_response("Tool finished."),
    ]));
    let engine = Engine::new(
        model,
        Arc::new(TestCapabilityHost::new(workspace.path())),
        Arc::new(
            SqliteRunStore::open(workspace.path().join("renoa.db")).expect("run store must open"),
        ),
        EngineConfig::default(),
    );
    let events = Arc::new(RecordingEventSink::default());
    let mut agent = Agent::new(engine, test_agent()).with_event_sink(events.clone());

    agent
        .prompt(test_command(), CancellationToken::new())
        .await
        .expect("prompt must complete");

    let expected = [
        AgentEvent::ToolExecutionStart { call: call.clone() },
        AgentEvent::ToolExecutionEnd {
            call: call.clone(),
            outcome: outcome.clone(),
        },
        AgentEvent::MessageStart {
            message: Message::Capability {
                call_id: call.call_id,
                name: call.name,
                outcome,
            },
        },
    ];
    assert!(
        events
            .events()
            .windows(expected.len())
            .any(|events| events == expected)
    );
}

#[tokio::test]
async fn event_sink_observes_lifecycle_end_after_model_failure() {
    let workspace = tempdir().expect("temporary workspace must be created");
    let model = Arc::new(ScriptedModel::from_steps(vec![ModelStep::Fail(
        "provider failed".to_owned(),
    )]));
    let engine = Engine::new(
        model,
        Arc::new(TestCapabilityHost::new(workspace.path())),
        Arc::new(
            SqliteRunStore::open(workspace.path().join("renoa.db")).expect("run store must open"),
        ),
        EngineConfig::default(),
    );
    let events = Arc::new(RecordingEventSink::default());
    let mut agent = Agent::new(engine, test_agent()).with_event_sink(events.clone());

    let result = agent.prompt(test_command(), CancellationToken::new()).await;

    assert!(matches!(result, Err(renoa_runtime::EngineError::Model(_))));
    assert!(matches!(
        events.events().as_slice(),
        [
            AgentEvent::AgentStart,
            AgentEvent::TurnStart,
            AgentEvent::MessageStart { .. },
            AgentEvent::MessageEnd { .. },
            AgentEvent::TurnEnd,
            AgentEvent::AgentEnd,
        ]
    ));
}

fn final_response(text: &str) -> ModelResponse {
    ModelResponse {
        text: text.to_owned(),
        capability_calls: Vec::new(),
        truncated: false,
    }
}

struct RecordingEventSink {
    events: Mutex<Vec<AgentEvent>>,
    cancel_on_update: Option<CancellationToken>,
}

impl Default for RecordingEventSink {
    fn default() -> Self {
        Self {
            events: Mutex::new(Vec::new()),
            cancel_on_update: None,
        }
    }
}

impl RecordingEventSink {
    fn cancelling(cancellation: CancellationToken) -> Self {
        Self {
            events: Mutex::new(Vec::new()),
            cancel_on_update: Some(cancellation),
        }
    }

    fn events(&self) -> Vec<AgentEvent> {
        self.events
            .lock()
            .expect("event sink lock must not be poisoned")
            .clone()
    }
}

impl AgentEventSink for RecordingEventSink {
    fn emit(&self, event: AgentEvent) -> BoxFuture<'_, ()> {
        Box::pin(async move {
            let should_cancel = matches!(event, AgentEvent::MessageUpdate { .. });
            self.events
                .lock()
                .expect("event sink lock must not be poisoned")
                .push(event);
            if should_cancel && let Some(cancellation) = &self.cancel_on_update {
                cancellation.cancel();
            }
        })
    }
}
