use renoa_core::{
    CommandEnvelope, CommandId, CommandInput, PrincipalId, ResolvedAgent, RunAdmission, RunId,
    RunStore, SurfaceRef, TargetRef,
};
use renoa_store_sqlite::SqliteRunStore;
use rusqlite::params;
use tempfile::tempdir;

#[tokio::test]
async fn opening_a_legacy_ledger_backfills_command_identity() {
    let workspace = tempdir().expect("temporary workspace must be created");
    let database_path = workspace.path().join("legacy.db");
    let command = command();
    let agent = agent();
    let original_run_id = RunId::new();
    let connection = rusqlite::Connection::open(&database_path).expect("legacy ledger must open");
    connection
        .execute_batch(
            "CREATE TABLE runs (
                run_id TEXT PRIMARY KEY,
                command_json TEXT NOT NULL,
                agent_json TEXT NOT NULL,
                status TEXT NOT NULL,
                terminal_json TEXT,
                next_sequence INTEGER NOT NULL,
                created_at_ms INTEGER NOT NULL,
                finished_at_ms INTEGER
            );",
        )
        .expect("legacy schema must be created");
    connection
        .execute(
            "INSERT INTO runs (
                run_id, command_json, agent_json, status, terminal_json,
                next_sequence, created_at_ms, finished_at_ms
             ) VALUES (?1, ?2, ?3, 'open', NULL, 0, 0, NULL)",
            params![
                original_run_id.to_string(),
                serde_json::to_string(&command).expect("command must serialize"),
                serde_json::to_string(&agent).expect("agent must serialize"),
            ],
        )
        .expect("legacy run must be inserted");
    drop(connection);

    let store = SqliteRunStore::open(&database_path).expect("legacy ledger must migrate");
    let retry = store
        .admit_run(command.clone(), agent.clone())
        .await
        .expect("same command must be admitted idempotently");
    assert_eq!(retry, RunAdmission::Existing(original_run_id));

    let mut changed = command;
    changed.input = CommandInput::Text {
        text: "changed".to_owned(),
    };
    let conflict = store
        .admit_run(changed, agent)
        .await
        .expect("changed command must produce a typed conflict");
    assert_eq!(conflict, RunAdmission::Conflict(original_run_id));
}

fn command() -> CommandEnvelope {
    CommandEnvelope {
        command_id: CommandId::new(),
        principal_id: PrincipalId::new(),
        surface: SurfaceRef::new("legacy-test"),
        target: TargetRef::new("local:legacy-test"),
        input: CommandInput::Text {
            text: "original".to_owned(),
        },
    }
}

fn agent() -> ResolvedAgent {
    ResolvedAgent {
        instructions: "Test legacy migration.".to_owned(),
        capability_grants: Vec::new(),
    }
}
