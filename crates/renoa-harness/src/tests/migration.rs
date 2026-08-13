use std::{num::NonZeroU32, sync::Arc};

use renoa_agent::{
    BoxFuture, ContentBlock, Message, ModelRequest, Tool, ToolCall, ToolError, ToolOutput,
    ToolSpec, ToolUpdates,
};
use rusqlite::params;
use tempfile::tempdir;
use uuid::Uuid;

use super::support::{NeverCalledModel, RecordingModel};
use crate::{
    Harness, HarnessError, OperationId, OperationRequest, OperationStatus, RequestId, RunNext,
    RuntimeProfile, SessionId, ToolBinding, ToolRecovery,
    schema::{initialize_v1_for_test, initialize_v2_for_test, open_connection},
};
use tokio_util::sync::CancellationToken;

#[test]
fn a_v2_database_adds_cancellation_storage_without_rebuilding_tools() {
    let directory = tempdir().expect("temporary directory");
    let database = directory.path().join("harness.sqlite3");
    let mut connection = open_connection(&database).expect("open v2 database");
    initialize_v2_for_test(&mut connection).expect("initialize v2 schema");
    assert!(tool_table_exists(&connection));
    assert!(!cancellation_table_exists(&connection));
    drop(connection);

    drop(Harness::open(&database).expect("migrate v2 harness"));

    let connection = open_connection(&database).expect("reopen migrated database");
    assert_eq!(pragma_version(&connection), 4);
    assert!(tool_table_exists(&connection));
    assert!(cancellation_table_exists(&connection));
}

#[tokio::test]
async fn a_v3_tool_operation_without_a_binding_identity_migrates_fail_closed() {
    let directory = tempdir().expect("temporary directory");
    let database = directory.path().join("harness.sqlite3");
    let session_id = SessionId::new();
    let operation_id = OperationId::from_uuid(Uuid::new_v4());
    let mut connection = open_connection(&database).expect("open v3 database");
    initialize_v2_for_test(&mut connection).expect("initialize v2 schema");
    connection
        .execute_batch(
            "CREATE TABLE cancellation_requests (
                cancellation_id TEXT PRIMARY KEY NOT NULL,
                session_id TEXT NOT NULL,
                operation_id TEXT NOT NULL,
                FOREIGN KEY (session_id, operation_id)
                    REFERENCES operations(session_id, operation_id)
             ) STRICT;
             CREATE INDEX cancellation_requests_operation
                ON cancellation_requests(operation_id);
             PRAGMA user_version = 3;",
        )
        .expect("upgrade fixture to v3");
    insert_v3_planned_tool(&connection, session_id, operation_id);
    drop(connection);

    let tool = Arc::new(NeverExecutedTool::new());
    let profile = RuntimeProfile::new(
        "legacy-tools-v1",
        Arc::new(NeverCalledModel),
        "Be precise.",
        NonZeroU32::new(2).expect("non-zero attempt limit"),
    )
    .with_tools(
        vec![ToolBinding::new(
            "read-file-v1",
            tool,
            ToolRecovery::NeverReplay,
        )],
        NonZeroU32::new(1).expect("non-zero tool-call limit"),
    )
    .expect("valid profile");
    let harness = Harness::open(&database).expect("migrate v3 harness");

    assert_eq!(
        harness
            .run_next(session_id, &profile)
            .await
            .expect_err("an unidentified legacy binding must not execute"),
        HarnessError::ToolBindingUnavailable {
            name: "read_file".to_owned(),
            revision: "legacy-tools-v1".to_owned(),
        }
    );
}

#[tokio::test]
async fn a_v1_active_model_operation_migrates_and_recovers_its_exact_request() {
    let directory = tempdir().expect("temporary directory");
    let database = directory.path().join("harness.sqlite3");
    let session_id = SessionId::new();
    let operation_id = OperationId::from_uuid(Uuid::new_v4());
    let effect_id = Uuid::new_v4();
    let settlement_token = Uuid::new_v4();
    let request = ModelRequest {
        system_prompt: "frozen v1 prompt".to_owned(),
        messages: vec![Message::user_text("continue old work")],
        tools: Vec::new(),
    };
    let mut connection = open_connection(&database).expect("open v1 database");
    initialize_v1_for_test(&mut connection).expect("initialize v1 schema");
    insert_v1_pending_operation(
        &connection,
        session_id,
        operation_id,
        effect_id,
        settlement_token,
        &request,
    );
    drop(connection);

    let model = Arc::new(RecordingModel::default());
    let profile = RuntimeProfile::new(
        "legacy-v1",
        model.clone(),
        "host prompt must not replace the frozen prompt",
        NonZeroU32::new(9).expect("non-zero attempt limit"),
    );
    let harness = Harness::open(&database).expect("migrate v1 harness");
    assert!(matches!(
        harness
            .run_next(session_id, &profile)
            .await
            .expect("recover migrated operation"),
        RunNext::Finished {
            operation_id: finished,
            ..
        } if finished == operation_id
    ));
    assert_eq!(model.requests(), vec![request]);
    assert_eq!(
        harness
            .inspect(session_id)
            .await
            .expect("inspect migrated session")
            .operations[0]
            .status,
        OperationStatus::Completed
    );
    drop(harness);

    let connection = open_connection(&database).expect("reopen migrated database");
    assert_eq!(pragma_version(&connection), 4);
    assert!(tool_table_exists(&connection));
    assert!(cancellation_table_exists(&connection));
}

#[tokio::test]
async fn v1_queued_and_failed_states_remain_readable_after_migration() {
    let directory = tempdir().expect("temporary directory");
    let database = directory.path().join("harness.sqlite3");
    let queued_session = SessionId::new();
    let failed_session = SessionId::new();
    let mut connection = open_connection(&database).expect("open v1 database");
    initialize_v1_for_test(&mut connection).expect("initialize v1 schema");
    insert_v1_terminal_or_queued(
        &connection,
        queued_session,
        OperationId::from_uuid(Uuid::new_v4()),
        "queued",
    );
    insert_v1_terminal_or_queued(
        &connection,
        failed_session,
        OperationId::from_uuid(Uuid::new_v4()),
        "failed",
    );
    drop(connection);

    let harness = Harness::open(&database).expect("migrate v1 harness");
    assert_eq!(
        harness
            .inspect(queued_session)
            .await
            .expect("inspect queued session")
            .operations[0]
            .status,
        OperationStatus::Queued
    );
    assert_eq!(
        harness
            .inspect(failed_session)
            .await
            .expect("inspect failed session")
            .operations[0]
            .status,
        OperationStatus::Failed
    );
}

fn insert_v1_pending_operation(
    connection: &rusqlite::Connection,
    session_id: SessionId,
    operation_id: OperationId,
    effect_id: Uuid,
    settlement_token: Uuid,
    request: &ModelRequest,
) {
    connection
        .execute(
            "INSERT INTO sessions (
                session_id, next_operation_position, active_operation_id,
                next_entry_sequence, next_output_sequence
             ) VALUES (?1, 1, NULL, 1, 0)",
            [session_id.to_string()],
        )
        .expect("insert v1 session");
    let state = serde_json::json!({
        "format_version": 1,
        "state": {
            "phase": "model_pending",
            "runtime_revision": "legacy-v1",
            "max_model_attempts": 2,
            "attempt_count": 1,
            "effect_id": effect_id,
            "settlement_token": settlement_token,
            "assistant_entry_id": Uuid::new_v4(),
            "output_id": Uuid::new_v4(),
        }
    });
    let admitted = OperationRequest::new(
        RequestId::new(),
        vec![ContentBlock::text("continue old work")],
    );
    connection
        .execute(
            "INSERT INTO operations (
                operation_id, session_id, request_id, position, request_json, state_json
             ) VALUES (?1, ?2, ?3, 0, ?4, ?5)",
            params![
                operation_id.to_string(),
                session_id.to_string(),
                admitted.request_id().to_string(),
                serde_json::to_string(&admitted).expect("serialize admission"),
                state.to_string(),
            ],
        )
        .expect("insert v1 operation");
    connection
        .execute(
            "INSERT INTO conversation_entries (
                entry_id, session_id, operation_id, sequence, message_json
             ) VALUES (?1, ?2, ?3, 0, ?4)",
            params![
                Uuid::new_v4().to_string(),
                session_id.to_string(),
                operation_id.to_string(),
                serde_json::to_string(&Message::user_text("continue old work"))
                    .expect("serialize user entry"),
            ],
        )
        .expect("insert v1 user entry");
    connection
        .execute(
            "INSERT INTO model_attempts (
                effect_id, operation_id, attempt_number, settlement_token,
                status, request_json, usage_json, error
             ) VALUES (?1, ?2, 1, ?3, 'pending', ?4, NULL, NULL)",
            params![
                effect_id.to_string(),
                operation_id.to_string(),
                settlement_token.to_string(),
                serde_json::to_string(request).expect("serialize model request"),
            ],
        )
        .expect("insert v1 model attempt");
    connection
        .execute(
            "UPDATE sessions SET active_operation_id = ?2 WHERE session_id = ?1",
            params![session_id.to_string(), operation_id.to_string()],
        )
        .expect("activate v1 operation");
}

fn insert_v1_terminal_or_queued(
    connection: &rusqlite::Connection,
    session_id: SessionId,
    operation_id: OperationId,
    phase: &str,
) {
    connection
        .execute(
            "INSERT INTO sessions (
                session_id, next_operation_position, active_operation_id,
                next_entry_sequence, next_output_sequence
             ) VALUES (?1, 1, NULL, 0, 0)",
            [session_id.to_string()],
        )
        .expect("insert v1 session");
    let admitted = OperationRequest::new(RequestId::new(), vec![ContentBlock::text("old")]);
    let state = serde_json::json!({"format_version": 1, "state": {"phase": phase}});
    connection
        .execute(
            "INSERT INTO operations (
                operation_id, session_id, request_id, position, request_json, state_json
             ) VALUES (?1, ?2, ?3, 0, ?4, ?5)",
            params![
                operation_id.to_string(),
                session_id.to_string(),
                admitted.request_id().to_string(),
                serde_json::to_string(&admitted).expect("serialize admission"),
                state.to_string(),
            ],
        )
        .expect("insert v1 operation");
}

fn insert_v3_planned_tool(
    connection: &rusqlite::Connection,
    session_id: SessionId,
    operation_id: OperationId,
) {
    let batch_id = Uuid::new_v4();
    let call = ToolCall {
        id: "call-1".to_owned(),
        name: "read_file".to_owned(),
        arguments: serde_json::json!({"path": "src/lib.rs"}),
        thought_signature: None,
        namespace: None,
    };
    let state = serde_json::json!({
        "format_version": 2,
        "state": {
            "phase": "need_tool",
            "progress": {
                "runtime": {
                    "revision": "legacy-tools-v1",
                    "system_prompt": "Be precise.",
                    "max_model_attempts": 2,
                    "max_tool_calls_per_step": 1,
                    "tools": [{
                        "spec": NeverExecutedTool::specification(),
                        "recovery": "never_replay"
                    }]
                },
                "model_attempts": 1
            },
            "batch": {
                "batch_id": batch_id,
                "next_index": 0,
                "call_count": 1
            }
        }
    });
    let request = OperationRequest::new(RequestId::new(), vec![ContentBlock::text("inspect it")]);
    connection
        .execute(
            "INSERT INTO sessions (
                session_id, next_operation_position, active_operation_id,
                next_entry_sequence, next_output_sequence
             ) VALUES (?1, 1, NULL, 0, 0)",
            [session_id.to_string()],
        )
        .expect("insert v3 session");
    connection
        .execute(
            "INSERT INTO operations (
                operation_id, session_id, request_id, position, request_json, state_json
             ) VALUES (?1, ?2, ?3, 0, ?4, ?5)",
            params![
                operation_id.to_string(),
                session_id.to_string(),
                request.request_id().to_string(),
                serde_json::to_string(&request).expect("serialize request"),
                state.to_string(),
            ],
        )
        .expect("insert v3 operation");
    connection
        .execute(
            "INSERT INTO tool_calls (
                operation_id, batch_id, source_index, result_entry_id, call_json,
                status, recovery, effect_id, settlement_token
             ) VALUES (?1, ?2, 0, ?3, ?4, 'planned', NULL, NULL, NULL)",
            params![
                operation_id.to_string(),
                batch_id.to_string(),
                Uuid::new_v4().to_string(),
                serde_json::to_string(&call).expect("serialize tool call"),
            ],
        )
        .expect("insert v3 tool call");
    connection
        .execute(
            "UPDATE sessions SET active_operation_id = ?2 WHERE session_id = ?1",
            params![session_id.to_string(), operation_id.to_string()],
        )
        .expect("activate v3 operation");
}

struct NeverExecutedTool {
    spec: ToolSpec,
}

impl NeverExecutedTool {
    fn new() -> Self {
        Self {
            spec: Self::specification(),
        }
    }

    fn specification() -> ToolSpec {
        ToolSpec {
            name: "read_file".to_owned(),
            description: "Read one file".to_owned(),
            input_schema: serde_json::json!({"type": "object"}),
        }
    }
}

impl Tool for NeverExecutedTool {
    fn spec(&self) -> &ToolSpec {
        &self.spec
    }

    fn execute(
        &self,
        _call: ToolCall,
        _cancellation: CancellationToken,
        _updates: ToolUpdates,
    ) -> BoxFuture<'_, Result<ToolOutput, ToolError>> {
        panic!("a legacy tool without a binding identity must not execute")
    }
}

fn pragma_version(connection: &rusqlite::Connection) -> u32 {
    connection
        .pragma_query_value(None, "user_version", |row| row.get(0))
        .expect("read schema version")
}

fn tool_table_exists(connection: &rusqlite::Connection) -> bool {
    connection
        .query_row(
            "SELECT EXISTS(
                SELECT 1 FROM sqlite_schema WHERE type = 'table' AND name = 'tool_calls'
             )",
            [],
            |row| row.get(0),
        )
        .expect("inspect tool table")
}

fn cancellation_table_exists(connection: &rusqlite::Connection) -> bool {
    connection
        .query_row(
            "SELECT EXISTS(
                SELECT 1 FROM sqlite_schema
                WHERE type = 'table' AND name = 'cancellation_requests'
             )",
            [],
            |row| row.get(0),
        )
        .expect("inspect cancellation table")
}
