use std::sync::Arc;

use crate::support::{ModelStep, ScriptedModel, TestCapabilityHost, test_agent, test_command};
use renoa_core::{CommandInput, ModelResponse, PrincipalId, RunEventKind, RunStore, TargetRef};
use renoa_runtime::{Engine, EngineConfig, EngineError};
use renoa_store_sqlite::SqliteRunStore;
use tempfile::tempdir;
use tokio_util::sync::CancellationToken;

#[tokio::test]
async fn completed_command_retry_returns_the_original_run_without_reexecution() {
    let workspace = tempdir().expect("temporary workspace must be created");
    let model = Arc::new(ScriptedModel::new(vec![ModelResponse {
        text: "Done.".to_owned(),
        capability_calls: Vec::new(),
        truncated: false,
    }]));
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
    let command = test_command();
    let agent = test_agent();

    let first = engine
        .run(command.clone(), agent.clone(), CancellationToken::new())
        .await
        .expect("first delivery must complete");
    let retry = engine
        .run(command, agent, CancellationToken::new())
        .await
        .expect("completed retry must return the original result");

    assert_eq!(retry, first);
    assert_eq!(model.requests().len(), 1);
    let transcript = store
        .load_transcript(first.run_id)
        .await
        .expect("original transcript must load");
    assert_eq!(
        transcript
            .events
            .iter()
            .filter(|event| matches!(event.kind, RunEventKind::RunStarted { .. }))
            .count(),
        1
    );
}

#[tokio::test]
async fn concurrent_duplicate_is_rejected_with_the_original_run_id() {
    let workspace = tempdir().expect("temporary workspace must be created");
    let database_path = workspace.path().join("renoa.db");
    let model = Arc::new(ScriptedModel::from_steps(vec![ModelStep::Pending]));
    let capabilities = Arc::new(TestCapabilityHost::new(workspace.path()));
    let first_store =
        Arc::new(SqliteRunStore::open(&database_path).expect("first store must open"));
    let second_store =
        Arc::new(SqliteRunStore::open(&database_path).expect("independent second store must open"));
    let first_engine = Arc::new(Engine::new(
        model.clone(),
        capabilities.clone(),
        first_store,
        EngineConfig::default(),
    ));
    let second_engine = Engine::new(
        model.clone(),
        capabilities,
        second_store,
        EngineConfig::default(),
    );
    let command = test_command();
    let agent = test_agent();
    let cancellation = CancellationToken::new();
    let first_task = tokio::spawn({
        let engine = first_engine.clone();
        let command = command.clone();
        let agent = agent.clone();
        let cancellation = cancellation.clone();
        async move { engine.run(command, agent, cancellation).await }
    });
    model.wait_for_request().await;
    let original_run_id = model.requests()[0].run_id;

    let duplicate = second_engine
        .run(command, agent, CancellationToken::new())
        .await
        .expect_err("an open duplicate must not execute");

    assert!(matches!(
        duplicate,
        EngineError::CommandAlreadyAdmitted(run_id) if run_id == original_run_id
    ));
    assert_eq!(model.requests().len(), 1);
    cancellation.cancel();
    assert!(matches!(
        first_task.await.expect("first engine task must settle"),
        Err(EngineError::Cancelled)
    ));
}

#[tokio::test]
async fn completed_command_remains_idempotent_after_store_reopen() {
    let workspace = tempdir().expect("temporary workspace must be created");
    let database_path = workspace.path().join("renoa.db");
    let command = test_command();
    let agent = test_agent();
    let first_model = Arc::new(ScriptedModel::new(vec![ModelResponse {
        text: "Persisted result.".to_owned(),
        capability_calls: Vec::new(),
        truncated: false,
    }]));
    let first_store = Arc::new(SqliteRunStore::open(&database_path).expect("run store must open"));
    let first_engine = Engine::new(
        first_model,
        Arc::new(TestCapabilityHost::new(workspace.path())),
        first_store,
        EngineConfig::default(),
    );
    let first = first_engine
        .run(command.clone(), agent.clone(), CancellationToken::new())
        .await
        .expect("first delivery must complete");
    drop(first_engine);

    let retry_model = Arc::new(ScriptedModel::new(Vec::new()));
    let reopened_engine = Engine::new(
        retry_model.clone(),
        Arc::new(TestCapabilityHost::new(workspace.path())),
        Arc::new(SqliteRunStore::open(&database_path).expect("run store must reopen")),
        EngineConfig::default(),
    );
    let retry = reopened_engine
        .run(command, agent, CancellationToken::new())
        .await
        .expect("retry after reopening must return the persisted result");

    assert_eq!(retry, first);
    assert!(retry_model.requests().is_empty());
}

#[tokio::test]
async fn reused_command_id_rejects_changed_command_or_agent_content() {
    let workspace = tempdir().expect("temporary workspace must be created");
    let model = Arc::new(ScriptedModel::new(vec![ModelResponse {
        text: "Original result.".to_owned(),
        capability_calls: Vec::new(),
        truncated: false,
    }]));
    let engine = Engine::new(
        model.clone(),
        Arc::new(TestCapabilityHost::new(workspace.path())),
        Arc::new(
            SqliteRunStore::open(workspace.path().join("renoa.db")).expect("run store must open"),
        ),
        EngineConfig::default(),
    );
    let command = test_command();
    let agent = test_agent();
    let original = engine
        .run(command.clone(), agent.clone(), CancellationToken::new())
        .await
        .expect("original command must complete");

    let mut changed_input = command.clone();
    changed_input.input = CommandInput::Text {
        text: "Different request.".to_owned(),
    };
    let mut changed_target = command.clone();
    changed_target.target = TargetRef::new("remote:other-workspace");
    let mut changed_principal = command.clone();
    changed_principal.principal_id = PrincipalId::new();
    let mut changed_instructions = agent.clone();
    changed_instructions.instructions = "Different instructions.".to_owned();
    let mut changed_grants = agent.clone();
    changed_grants.capability_grants = vec!["read_file".to_owned()];
    let conflicts = [
        (changed_input, agent.clone()),
        (changed_target, agent.clone()),
        (changed_principal, agent.clone()),
        (command.clone(), changed_instructions),
        (command, changed_grants),
    ];

    for (changed_command, changed_agent) in conflicts {
        let error = engine
            .run(changed_command, changed_agent, CancellationToken::new())
            .await
            .expect_err("changed content under one command id must conflict");
        assert!(matches!(
            error,
            EngineError::CommandConflict(run_id) if run_id == original.run_id
        ));
    }
    assert_eq!(model.requests().len(), 1);
}
