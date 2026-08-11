#[path = "kernel_turn/agent.rs"]
mod agent;
#[path = "kernel_turn/agent_events.rs"]
mod agent_events;
#[path = "kernel_turn/agent_resolution.rs"]
mod agent_resolution;
#[path = "kernel_turn/cancellation.rs"]
mod cancellation;
#[path = "kernel_turn/capability_execution.rs"]
mod capability_execution;
#[path = "kernel_turn/idempotency.rs"]
mod idempotency;
mod support;

use std::{
    fs,
    sync::{Arc, Mutex},
    time::Duration,
};

use renoa_core::{
    BoxFuture, CapabilityCall, CapabilityHost, CapabilityOutcome, CapabilityRequest,
    CapabilitySpec, Message, ModelResponse, ResolvedAgent, RunEventKind, RunStatus, RunStore,
    RunTranscript, TerminalState,
};
use renoa_runtime::{Engine, EngineConfig, EngineError};
use renoa_store_sqlite::SqliteRunStore;
use support::{
    ModelStep, ScriptedModel, TestCapabilityHost, event_name, scripted_responses, test_agent,
    test_command,
};
use tempfile::tempdir;
use tokio_util::sync::CancellationToken;

#[tokio::test]
async fn completes_a_durable_read_edit_exec_turn() {
    let workspace = tempdir().expect("temporary workspace must be created");
    let file_path = workspace.path().join("hello.txt");
    fs::write(&file_path, "first line\n").expect("fixture must be written");
    let database_path = workspace.path().join("renoa.db");

    let model = Arc::new(ScriptedModel::new(scripted_responses()));
    let capabilities = Arc::new(TestCapabilityHost::new(workspace.path()));
    let store = Arc::new(SqliteRunStore::open(&database_path).expect("run store must open"));
    let engine = Engine::new(
        model.clone(),
        capabilities,
        store.clone(),
        EngineConfig {
            max_model_rounds: 8,
            ..EngineConfig::default()
        },
    );
    let command = test_command();

    let result = engine
        .run(command.clone(), test_agent(), CancellationToken::new())
        .await
        .expect("agent turn must complete");

    assert_eq!(result.output, "Added the second line and verified it.");
    assert_eq!(result.model_rounds, 4);
    assert_eq!(
        fs::read_to_string(&file_path).expect("edited fixture must be readable"),
        "first line\nsecond line\n"
    );

    let requests = model.requests();
    assert_eq!(requests.len(), 4);
    assert_eq!(
        requests
            .iter()
            .map(|request| request.messages.len())
            .collect::<Vec<_>>(),
        vec![2, 4, 6, 8]
    );
    assert_eq!(
        requests[0].messages,
        vec![
            Message::System {
                text: test_agent().instructions,
            },
            Message::User {
                text: command.input.text().to_owned(),
            },
        ]
    );

    let transcript_before_restart = store
        .load_transcript(result.run_id)
        .await
        .expect("completed transcript must load");
    assert_transcript_contract(&transcript_before_restart);
    assert!(
        store
            .finish_run(
                result.run_id,
                TerminalState::Failed {
                    error: "late terminal writer".to_owned(),
                },
            )
            .await
            .is_err(),
        "a second terminal transition must lose the compare-and-set"
    );

    drop(engine);
    drop(store);
    let reopened_store = SqliteRunStore::open(&database_path).expect("run store must reopen");
    let transcript_after_restart = reopened_store
        .load_transcript(result.run_id)
        .await
        .expect("transcript must survive reopening the store");
    assert_eq!(transcript_after_restart, transcript_before_restart);
}

#[tokio::test]
async fn truncated_model_response_never_executes_capabilities() {
    let workspace = tempdir().expect("temporary workspace must be created");
    let mut truncated = support::tool_response(
        "unsafe-call",
        "read_file",
        serde_json::json!({ "path": "hello.txt" }),
    );
    truncated.truncated = true;
    let model = Arc::new(ScriptedModel::new(vec![
        truncated,
        renoa_core::ModelResponse {
            text: "Retried safely.".to_owned(),
            capability_calls: Vec::new(),
            truncated: false,
        },
    ]));
    let capabilities = Arc::new(TestCapabilityHost::new(workspace.path()));
    let store = Arc::new(
        SqliteRunStore::open(workspace.path().join("renoa.db")).expect("run store must open"),
    );
    let engine = Engine::new(
        model.clone(),
        capabilities.clone(),
        store,
        EngineConfig::default(),
    );

    let result = engine
        .run(test_command(), test_agent(), CancellationToken::new())
        .await
        .expect("agent should recover from a truncated capability call");

    assert_eq!(result.output, "Retried safely.");
    assert_eq!(capabilities.executions(), Vec::<String>::new());
    assert!(matches!(
        &model.requests()[1].messages[3],
        Message::Capability { outcome, .. }
            if outcome.is_error
                && outcome.model_view["error"]
                    .as_str()
                    .is_some_and(|message| message.contains("truncated"))
    ));
}

#[tokio::test]
async fn capabilities_execute_and_reenter_context_in_source_order() {
    let workspace = tempdir().expect("temporary workspace must be created");
    let model = Arc::new(ScriptedModel::new(vec![
        ModelResponse {
            text: String::new(),
            capability_calls: vec![
                delayed_call("first", 90),
                delayed_call("second", 10),
                delayed_call("third", 50),
            ],
            truncated: false,
        },
        ModelResponse {
            text: "All calls completed.".to_owned(),
            capability_calls: Vec::new(),
            truncated: false,
        },
    ]));
    let capabilities = Arc::new(DelayedCapabilityHost::new());
    let store = Arc::new(
        SqliteRunStore::open(workspace.path().join("renoa.db")).expect("run store must open"),
    );
    let engine = Engine::new(
        model.clone(),
        capabilities.clone(),
        store,
        EngineConfig::default(),
    );

    tokio::time::timeout(
        Duration::from_secs(2),
        engine.run(
            test_command(),
            ResolvedAgent {
                instructions: test_agent().instructions,
                capability_grants: vec!["delay".to_owned()],
            },
            CancellationToken::new(),
        ),
    )
    .await
    .expect("capability batch must not deadlock")
    .expect("capability turn must complete");

    assert_eq!(capabilities.completions(), vec!["first", "second", "third"]);
    let requests = model.requests();
    let reinjected_call_ids = requests[1].messages[3..]
        .iter()
        .map(|message| match message {
            Message::Capability { call_id, .. } => call_id.as_str(),
            Message::System { .. } | Message::User { .. } | Message::Assistant { .. } => {
                panic!("only capability results should follow the assistant call batch")
            }
        })
        .collect::<Vec<_>>();
    assert_eq!(reinjected_call_ids, vec!["first", "second", "third"]);
}

#[tokio::test]
async fn cancellation_during_model_request_is_durable() {
    let workspace = tempdir().expect("temporary workspace must be created");
    let model = Arc::new(ScriptedModel::from_steps(vec![ModelStep::Pending]));
    let capabilities = Arc::new(TestCapabilityHost::new(workspace.path()));
    let store = Arc::new(
        SqliteRunStore::open(workspace.path().join("renoa.db")).expect("run store must open"),
    );
    let engine = Engine::new(
        model.clone(),
        capabilities,
        store.clone(),
        EngineConfig::default(),
    );
    let cancellation = CancellationToken::new();
    let canceller = tokio::spawn({
        let model = model.clone();
        let cancellation = cancellation.clone();
        async move {
            model.wait_for_request().await;
            cancellation.cancel();
        }
    });

    let error = tokio::time::timeout(
        Duration::from_secs(2),
        engine.run(test_command(), test_agent(), cancellation),
    )
    .await
    .expect("cancellation must settle the run")
    .expect_err("cancelled run must not produce a successful answer");
    canceller.await.expect("cancellation task must finish");

    assert!(matches!(error, EngineError::Cancelled));
    let transcript = store
        .load_transcript(model.requests()[0].run_id)
        .await
        .expect("cancelled transcript must load");
    assert_eq!(transcript.run.status, RunStatus::Cancelled);
    assert_eq!(
        transcript.run.terminal,
        Some(TerminalState::Cancelled {
            reason: "caller cancelled the run".to_owned(),
        })
    );
    assert_eq!(event_names(&transcript), terminal_event_names());
}

#[tokio::test]
async fn model_failure_is_durable() {
    let workspace = tempdir().expect("temporary workspace must be created");
    let model = Arc::new(ScriptedModel::from_steps(vec![ModelStep::Fail(
        "provider unavailable".to_owned(),
    )]));
    let capabilities = Arc::new(TestCapabilityHost::new(workspace.path()));
    let store = Arc::new(
        SqliteRunStore::open(workspace.path().join("renoa.db")).expect("run store must open"),
    );
    let engine = Engine::new(
        model.clone(),
        capabilities,
        store.clone(),
        EngineConfig::default(),
    );

    let error = engine
        .run(test_command(), test_agent(), CancellationToken::new())
        .await
        .expect_err("model failure must fail the run");

    assert!(matches!(error, EngineError::Model(_)));
    let transcript = store
        .load_transcript(model.requests()[0].run_id)
        .await
        .expect("failed transcript must load");
    assert_eq!(transcript.run.status, RunStatus::Failed);
    assert!(matches!(
        transcript.run.terminal,
        Some(TerminalState::Failed { ref error }) if error.contains("provider unavailable")
    ));
    assert_eq!(event_names(&transcript), terminal_event_names());
}

#[tokio::test]
async fn model_round_limit_is_durable() {
    let workspace = tempdir().expect("temporary workspace must be created");
    let model = Arc::new(ScriptedModel::new(vec![
        support::tool_response(
            "round-one",
            "read_file",
            serde_json::json!({ "path": "missing.txt" }),
        ),
        support::tool_response(
            "round-two",
            "read_file",
            serde_json::json!({ "path": "missing.txt" }),
        ),
    ]));
    let capabilities = Arc::new(TestCapabilityHost::new(workspace.path()));
    let store = Arc::new(
        SqliteRunStore::open(workspace.path().join("renoa.db")).expect("run store must open"),
    );
    let engine = Engine::new(
        model.clone(),
        capabilities.clone(),
        store.clone(),
        EngineConfig {
            max_model_rounds: 2,
            ..EngineConfig::default()
        },
    );

    let error = engine
        .run(test_command(), test_agent(), CancellationToken::new())
        .await
        .expect_err("round exhaustion must fail the run");

    assert!(matches!(error, EngineError::RoundLimit(2)));
    assert_eq!(model.requests().len(), 2);
    assert_eq!(capabilities.executions(), vec!["round-one", "round-two"]);
    let transcript = store
        .load_transcript(model.requests()[0].run_id)
        .await
        .expect("round-limited transcript must load");
    assert_eq!(transcript.run.status, RunStatus::Failed);
    assert!(matches!(
        transcript.run.terminal,
        Some(TerminalState::Failed { ref error }) if error.contains("round limit of 2")
    ));
    assert_eq!(transcript.events.len(), 10);
    assert!(matches!(
        transcript.events.last().map(|event| &event.kind),
        Some(RunEventKind::RunTerminated {
            terminal: TerminalState::Failed { .. }
        })
    ));
}

struct DelayedCapabilityHost {
    completions: Mutex<Vec<String>>,
}

impl DelayedCapabilityHost {
    fn new() -> Self {
        Self {
            completions: Mutex::new(Vec::new()),
        }
    }

    fn completions(&self) -> Vec<String> {
        self.completions
            .lock()
            .expect("completion-order lock must not be poisoned")
            .clone()
    }
}

impl CapabilityHost for DelayedCapabilityHost {
    fn specs(&self) -> Vec<CapabilitySpec> {
        vec![CapabilitySpec {
            name: "delay".to_owned(),
            description: "Complete after a test-controlled delay".to_owned(),
            input_schema: serde_json::json!({ "type": "object" }),
        }]
    }

    fn execute(
        &self,
        request: CapabilityRequest,
        _cancellation: CancellationToken,
    ) -> BoxFuture<'_, CapabilityOutcome> {
        Box::pin(async move {
            let delay_ms = request.call.arguments["delayMs"]
                .as_u64()
                .expect("delay capability requires delayMs");
            tokio::time::sleep(Duration::from_millis(delay_ms)).await;
            self.completions
                .lock()
                .expect("completion-order lock must not be poisoned")
                .push(request.call.call_id.clone());
            CapabilityOutcome {
                model_view: serde_json::json!({ "callId": request.call.call_id }),
                is_error: false,
            }
        })
    }
}

fn delayed_call(call_id: &str, delay_ms: u64) -> CapabilityCall {
    CapabilityCall {
        call_id: call_id.to_owned(),
        name: "delay".to_owned(),
        arguments: serde_json::json!({ "delayMs": delay_ms }),
    }
}

fn event_names(transcript: &RunTranscript) -> Vec<&'static str> {
    transcript
        .events
        .iter()
        .map(|event| event_name(&event.kind))
        .collect()
}

fn terminal_event_names() -> Vec<&'static str> {
    vec!["run_started", "model_requested", "run_terminated"]
}

fn assert_transcript_contract(transcript: &RunTranscript) {
    assert_eq!(transcript.run.status, RunStatus::Completed);
    assert_eq!(
        transcript.run.terminal,
        Some(TerminalState::Completed {
            output: "Added the second line and verified it.".to_owned(),
        })
    );
    assert_eq!(
        transcript
            .events
            .iter()
            .map(|event| event.sequence)
            .collect::<Vec<_>>(),
        (0..16).collect::<Vec<_>>()
    );
    assert_eq!(
        transcript
            .events
            .iter()
            .map(|event| event_name(&event.kind))
            .collect::<Vec<_>>(),
        vec![
            "run_started",
            "model_requested",
            "model_responded",
            "capability_requested",
            "capability_completed",
            "model_requested",
            "model_responded",
            "capability_requested",
            "capability_completed",
            "model_requested",
            "model_responded",
            "capability_requested",
            "capability_completed",
            "model_requested",
            "model_responded",
            "run_terminated",
        ]
    );
}
