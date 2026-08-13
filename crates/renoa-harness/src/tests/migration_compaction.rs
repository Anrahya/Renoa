use std::{num::NonZeroU32, sync::Arc};

use renoa_agent::ContentBlock;
use rusqlite::params;
use tempfile::tempdir;
use uuid::Uuid;

use super::support::{FixedResponseModel, response_with_usage};
use crate::{
    Harness, OperationId, OperationRequest, RequestId, RunNext, RuntimeProfile, SessionId,
    schema::{initialize_v4_for_test, open_connection},
};

#[tokio::test]
async fn a_v4_queued_operation_migrates_to_the_checkpoint_schema_and_runs() {
    let directory = tempdir().expect("temporary directory");
    let database = directory.path().join("harness.sqlite3");
    let session_id = SessionId::new();
    let operation_id = OperationId::from_uuid(Uuid::new_v4());
    let request = OperationRequest::new(RequestId::new(), vec![ContentBlock::text("continue")]);
    let mut connection = open_connection(&database).expect("open v4 database");
    initialize_v4_for_test(&mut connection).expect("initialize v4 schema");
    connection
        .execute(
            "INSERT INTO sessions (
                session_id, next_operation_position, active_operation_id,
                next_entry_sequence, next_output_sequence
             ) VALUES (?1, 1, NULL, 0, 0)",
            [session_id.to_string()],
        )
        .expect("insert v4 session");
    connection
        .execute(
            "INSERT INTO operations (
                operation_id, session_id, request_id, position, request_json, state_json
             ) VALUES (?1, ?2, ?3, 0, ?4, ?5)",
            params![
                operation_id.to_string(),
                session_id.to_string(),
                request.request_id().to_string(),
                serde_json::to_string(&request).expect("encode request"),
                serde_json::json!({"format_version": 3, "state": {"phase": "queued"}}).to_string(),
            ],
        )
        .expect("insert v4 operation");
    drop(connection);

    let harness = Harness::open(&database).expect("migrate v4 harness");
    let profile = RuntimeProfile::new(
        "coding-v1",
        Arc::new(FixedResponseModel(response_with_usage())),
        "Be precise.",
        NonZeroU32::new(1).expect("non-zero model attempts"),
    );
    assert!(matches!(
        harness
            .run_next(session_id, &profile)
            .await
            .expect("run migrated operation"),
        RunNext::Finished {
            operation_id: finished,
            ..
        } if finished == operation_id
    ));
    drop(harness);

    let connection = open_connection(&database).expect("reopen migrated database");
    assert_eq!(
        connection
            .pragma_query_value(None, "user_version", |row| row.get::<_, u32>(0))
            .expect("read schema version"),
        5
    );
    assert!(table_exists(&connection, "context_checkpoints"));
    assert!(table_exists(&connection, "compaction_attempts"));
}

fn table_exists(connection: &rusqlite::Connection, table: &str) -> bool {
    connection
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = ?1)",
            [table],
            |row| row.get(0),
        )
        .expect("inspect table")
}
