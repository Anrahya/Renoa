use std::sync::Arc;

use crate::support::{ScriptedModel, test_agent, test_command};
use renoa_core::{
    BoxFuture, CapabilityCall, CapabilityHost, CapabilityOutcome, CapabilityRequest,
    CapabilitySpec, Message, ModelResponse, RunEventKind, RunStatus, RunStore,
};
use renoa_runtime::{Engine, EngineConfig, EngineError};
use renoa_store_sqlite::SqliteRunStore;
use tempfile::tempdir;
use tokio_util::sync::CancellationToken;

#[tokio::test]
async fn completed_capability_is_durable_before_a_later_capability_panics() {
    let workspace = tempdir().expect("temporary workspace must be created");
    let model = Arc::new(ScriptedModel::new(vec![ModelResponse {
        text: String::new(),
        capability_calls: vec![test_call("first"), test_call("second")],
        truncated: false,
    }]));
    let store = Arc::new(
        SqliteRunStore::open(workspace.path().join("renoa.db")).expect("run store must open"),
    );
    let engine = Engine::new(
        model.clone(),
        Arc::new(PanicOnSecondCapability),
        store.clone(),
        EngineConfig::default(),
    );
    let mut agent = test_agent();
    agent.capability_grants = vec!["test".to_owned()];

    let error = engine
        .run(test_command(), agent, CancellationToken::new())
        .await
        .expect_err("a panicking capability adapter must fail the run");

    assert!(matches!(error, EngineError::CapabilityTask(_)));
    let transcript = store
        .load_transcript(model.requests()[0].run_id)
        .await
        .expect("failed transcript must load");
    assert_eq!(transcript.run.status, RunStatus::Failed);
    assert!(transcript.events.iter().any(|event| matches!(
        &event.kind,
        RunEventKind::CapabilityCompleted {
            ordinal: 0,
            call_id,
            outcome,
        } if call_id == "first" && !outcome.is_error
    )));
    assert!(!transcript.events.iter().any(|event| matches!(
        event.kind,
        RunEventKind::CapabilityCompleted { ordinal: 1, .. }
    )));
}

#[tokio::test]
async fn oversized_capability_batch_fails_before_any_capability_executes() {
    let workspace = tempdir().expect("temporary workspace must be created");
    let model = Arc::new(ScriptedModel::new(vec![ModelResponse {
        text: String::new(),
        capability_calls: vec![test_call("first"), test_call("second")],
        truncated: false,
    }]));
    let store = Arc::new(
        SqliteRunStore::open(workspace.path().join("renoa.db")).expect("run store must open"),
    );
    let engine = Engine::new(
        model.clone(),
        Arc::new(PanicOnSecondCapability),
        store.clone(),
        EngineConfig {
            max_model_rounds: 32,
            max_capability_calls_per_response: 1,
        },
    );
    let mut agent = test_agent();
    agent.capability_grants = vec!["test".to_owned()];

    let error = engine
        .run(test_command(), agent, CancellationToken::new())
        .await
        .expect_err("oversized capability batch must fail");

    assert!(matches!(
        error,
        EngineError::CapabilityBatchTooLarge {
            actual: 2,
            limit: 1,
        }
    ));
    let transcript = store
        .load_transcript(model.requests()[0].run_id)
        .await
        .expect("failed transcript must load");
    assert_eq!(transcript.run.status, RunStatus::Failed);
    assert!(!transcript.events.iter().any(|event| matches!(
        event.kind,
        RunEventKind::CapabilityRequested { .. } | RunEventKind::CapabilityCompleted { .. }
    )));
}

#[tokio::test]
async fn capability_host_can_route_on_the_command_target() {
    let workspace = tempdir().expect("temporary workspace must be created");
    let model = Arc::new(ScriptedModel::new(vec![
        ModelResponse {
            text: String::new(),
            capability_calls: vec![test_call("route")],
            truncated: false,
        },
        ModelResponse {
            text: "Routed.".to_owned(),
            capability_calls: Vec::new(),
            truncated: false,
        },
    ]));
    let engine = Engine::new(
        model.clone(),
        Arc::new(TargetEchoCapability),
        Arc::new(
            SqliteRunStore::open(workspace.path().join("renoa.db")).expect("run store must open"),
        ),
        EngineConfig::default(),
    );
    let command = test_command();
    let expected_target = "local:test-workspace";
    let mut agent = test_agent();
    agent.capability_grants = vec!["test".to_owned()];

    engine
        .run(command, agent, CancellationToken::new())
        .await
        .expect("target-aware capability turn must complete");

    assert!(matches!(
        &model.requests()[1].messages[3],
        Message::Capability { outcome, .. }
            if outcome.model_view["target"] == expected_target
    ));
}

fn test_call(call_id: &str) -> CapabilityCall {
    CapabilityCall {
        call_id: call_id.to_owned(),
        name: "test".to_owned(),
        arguments: serde_json::json!({}),
    }
}

struct PanicOnSecondCapability;

struct TargetEchoCapability;

impl CapabilityHost for PanicOnSecondCapability {
    fn specs(&self) -> Vec<CapabilitySpec> {
        vec![CapabilitySpec {
            name: "test".to_owned(),
            description: "Succeeds once, then panics".to_owned(),
            input_schema: serde_json::json!({ "type": "object" }),
        }]
    }

    fn execute(
        &self,
        request: CapabilityRequest,
        _cancellation: CancellationToken,
    ) -> BoxFuture<'_, CapabilityOutcome> {
        Box::pin(async move {
            assert_ne!(request.ordinal, 1, "broken capability adapter");
            CapabilityOutcome {
                model_view: serde_json::json!({ "completed": request.call.call_id }),
                is_error: false,
            }
        })
    }
}

impl CapabilityHost for TargetEchoCapability {
    fn specs(&self) -> Vec<CapabilitySpec> {
        PanicOnSecondCapability.specs()
    }

    fn execute(
        &self,
        request: CapabilityRequest,
        _cancellation: CancellationToken,
    ) -> BoxFuture<'_, CapabilityOutcome> {
        Box::pin(async move {
            CapabilityOutcome {
                model_view: serde_json::json!({ "target": request.target.as_str() }),
                is_error: false,
            }
        })
    }
}
