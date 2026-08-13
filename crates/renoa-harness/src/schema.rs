use std::path::Path;

use renoa_agent::ModelRequest;
use rusqlite::{Connection, OptionalExtension, Transaction, TransactionBehavior, params};
use serde::Deserialize;
use uuid::Uuid;

use crate::{
    HarnessError,
    state::{FailureKind, FrozenRuntime, OperationProgress, StoredOperationState, StoredState},
};

const SCHEMA_VERSION: u32 = 4;
const SCHEMA_V1: &str = "CREATE TABLE sessions (
        session_id TEXT PRIMARY KEY NOT NULL,
        next_operation_position INTEGER NOT NULL CHECK (next_operation_position >= 0),
        active_operation_id TEXT,
        next_entry_sequence INTEGER NOT NULL CHECK (next_entry_sequence >= 0),
        next_output_sequence INTEGER NOT NULL CHECK (next_output_sequence >= 0),
        FOREIGN KEY (session_id, active_operation_id)
            REFERENCES operations(session_id, operation_id)
            DEFERRABLE INITIALLY DEFERRED
     ) STRICT;

     CREATE TABLE operations (
        operation_id TEXT PRIMARY KEY NOT NULL,
        session_id TEXT NOT NULL REFERENCES sessions(session_id),
        request_id TEXT NOT NULL,
        position INTEGER NOT NULL CHECK (position >= 0),
        request_json TEXT NOT NULL,
        state_json TEXT NOT NULL,
        UNIQUE (session_id, request_id),
        UNIQUE (session_id, position),
        UNIQUE (session_id, operation_id)
     ) STRICT;

     CREATE TABLE conversation_entries (
        entry_id TEXT PRIMARY KEY NOT NULL,
        session_id TEXT NOT NULL REFERENCES sessions(session_id),
        operation_id TEXT NOT NULL,
        sequence INTEGER NOT NULL CHECK (sequence >= 0),
        message_json TEXT NOT NULL,
        UNIQUE (session_id, sequence),
        FOREIGN KEY (session_id, operation_id)
            REFERENCES operations(session_id, operation_id)
     ) STRICT;

     CREATE TABLE model_attempts (
        effect_id TEXT PRIMARY KEY NOT NULL,
        operation_id TEXT NOT NULL REFERENCES operations(operation_id),
        attempt_number INTEGER NOT NULL CHECK (attempt_number > 0),
        settlement_token TEXT NOT NULL,
        status TEXT NOT NULL CHECK (
            status IN ('pending', 'completed', 'outcome_unknown')
        ),
        request_json TEXT CHECK (
            (status = 'pending') = (request_json IS NOT NULL)
        ),
        usage_json TEXT,
        error TEXT,
        UNIQUE (operation_id, attempt_number)
     ) STRICT;

     CREATE TABLE outputs (
        output_id TEXT PRIMARY KEY NOT NULL,
        session_id TEXT NOT NULL REFERENCES sessions(session_id),
        operation_id TEXT NOT NULL,
        sequence INTEGER NOT NULL CHECK (sequence >= 0),
        outcome_json TEXT NOT NULL,
        UNIQUE (session_id, sequence),
        UNIQUE (operation_id),
        FOREIGN KEY (session_id, operation_id)
            REFERENCES operations(session_id, operation_id)
     ) STRICT;";

const SCHEMA_V2: &str = "CREATE TABLE tool_calls (
        operation_id TEXT NOT NULL REFERENCES operations(operation_id),
        batch_id TEXT NOT NULL,
        source_index INTEGER NOT NULL CHECK (source_index >= 0),
        result_entry_id TEXT NOT NULL UNIQUE,
        call_json TEXT NOT NULL,
        status TEXT NOT NULL CHECK (
            status IN ('planned', 'pending', 'outcome_unknown')
        ),
        recovery TEXT CHECK (
            recovery IN ('safe_to_replay', 'never_replay')
        ),
        effect_id TEXT UNIQUE,
        settlement_token TEXT,
        PRIMARY KEY (operation_id, batch_id, source_index),
        CHECK (
            (status = 'planned' AND recovery IS NULL
                AND effect_id IS NULL AND settlement_token IS NULL)
            OR
            (status != 'planned' AND recovery IS NOT NULL
                AND effect_id IS NOT NULL AND settlement_token IS NOT NULL)
        )
     ) STRICT;";

const SCHEMA_V3: &str = "CREATE TABLE cancellation_requests (
        cancellation_id TEXT PRIMARY KEY NOT NULL,
        session_id TEXT NOT NULL,
        operation_id TEXT NOT NULL,
        FOREIGN KEY (session_id, operation_id)
            REFERENCES operations(session_id, operation_id)
     ) STRICT;

     CREATE INDEX cancellation_requests_operation
        ON cancellation_requests(operation_id);";

pub(crate) fn initialize(connection: &mut Connection) -> Result<(), HarnessError> {
    let version = connection
        .pragma_query_value(None, "user_version", |row| row.get::<_, u32>(0))
        .map_err(sqlite_error)?;
    if version > SCHEMA_VERSION {
        return Err(HarnessError::UnsupportedSchema {
            found: version,
            supported: SCHEMA_VERSION,
        });
    }
    if version == SCHEMA_VERSION {
        return Ok(());
    }

    let transaction = connection
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(sqlite_error)?;
    if version == 0 {
        transaction.execute_batch(SCHEMA_V1).map_err(sqlite_error)?;
    }
    if version <= 1 {
        migrate_v1_to_v2(&transaction)?;
    }
    if version <= 2 {
        migrate_v2_to_v3(&transaction)?;
    }
    migrate_v3_to_v4(&transaction)?;
    transaction
        .pragma_update(None, "user_version", SCHEMA_VERSION)
        .map_err(sqlite_error)?;
    transaction.commit().map_err(sqlite_error)
}

fn migrate_v2_to_v3(transaction: &Transaction<'_>) -> Result<(), HarnessError> {
    transaction.execute_batch(SCHEMA_V3).map_err(sqlite_error)
}

fn migrate_v3_to_v4(transaction: &Transaction<'_>) -> Result<(), HarnessError> {
    let operations = {
        let mut statement = transaction
            .prepare("SELECT operation_id, state_json FROM operations")
            .map_err(sqlite_error)?;
        let rows = statement
            .query_map([], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
            })
            .map_err(sqlite_error)?;
        rows.collect::<Result<Vec<_>, _>>().map_err(sqlite_error)?
    };
    for (operation_id, old_json) in operations {
        let version = serde_json::from_str::<StoredStateVersion>(&old_json)
            .map_err(json_error)?
            .format_version;
        let new_json = match version {
            1 => migrate_state_v1(transaction, &old_json)?,
            2 => migrate_state_v2(&old_json)?,
            crate::state::STORED_STATE_VERSION => continue,
            unsupported => {
                return Err(HarnessError::Corrupt(format!(
                    "cannot migrate operation state version {unsupported}"
                )));
            }
        };
        let changed = transaction
            .execute(
                "UPDATE operations SET state_json = ?2
                 WHERE operation_id = ?1 AND state_json = ?3",
                params![operation_id, new_json, old_json],
            )
            .map_err(sqlite_error)?;
        if changed != 1 {
            return Err(HarnessError::Corrupt(
                "operation-state migration compare-and-set failed".to_owned(),
            ));
        }
    }
    Ok(())
}

#[cfg(test)]
pub(crate) fn initialize_v1_for_test(connection: &mut Connection) -> Result<(), HarnessError> {
    let transaction = connection
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(sqlite_error)?;
    transaction.execute_batch(SCHEMA_V1).map_err(sqlite_error)?;
    transaction
        .pragma_update(None, "user_version", 1)
        .map_err(sqlite_error)?;
    transaction.commit().map_err(sqlite_error)
}

#[cfg(test)]
pub(crate) fn initialize_v2_for_test(connection: &mut Connection) -> Result<(), HarnessError> {
    let transaction = connection
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(sqlite_error)?;
    transaction.execute_batch(SCHEMA_V1).map_err(sqlite_error)?;
    transaction.execute_batch(SCHEMA_V2).map_err(sqlite_error)?;
    transaction
        .pragma_update(None, "user_version", 2)
        .map_err(sqlite_error)?;
    transaction.commit().map_err(sqlite_error)
}

fn migrate_v1_to_v2(transaction: &Transaction<'_>) -> Result<(), HarnessError> {
    transaction.execute_batch(SCHEMA_V2).map_err(sqlite_error)
}

fn migrate_state_v2(value: &str) -> Result<String, HarnessError> {
    let old: StoredState = serde_json::from_str(value).map_err(json_error)?;
    if old.format_version() != 2 {
        return Err(HarnessError::Corrupt(format!(
            "v2 database contains operation state version {}",
            old.format_version()
        )));
    }
    serde_json::to_string(&StoredState::from_state(old.into_state())).map_err(json_error)
}

fn migrate_state_v1(transaction: &Transaction<'_>, value: &str) -> Result<String, HarnessError> {
    let old: StoredStateV1 = serde_json::from_str(value).map_err(json_error)?;
    if old.format_version != 1 {
        return Err(HarnessError::Corrupt(format!(
            "v1 database contains operation state version {}",
            old.format_version
        )));
    }
    let state = match old.state {
        StoredOperationStateV1::Queued => StoredOperationState::Queued,
        StoredOperationStateV1::NeedModel {
            runtime_revision,
            system_prompt,
            max_model_attempts,
            attempt_count,
        } => StoredOperationState::NeedModel {
            progress: OperationProgress {
                runtime: model_only_runtime(runtime_revision, system_prompt, max_model_attempts),
                model_attempts: attempt_count,
            },
        },
        StoredOperationStateV1::ModelPending {
            runtime_revision,
            max_model_attempts,
            attempt_count,
            effect_id,
            settlement_token,
            assistant_entry_id,
            output_id,
        } => {
            let request = load_pending_v1_request(transaction, effect_id)?;
            if !request.tools.is_empty() {
                return Err(HarnessError::Corrupt(
                    "v1 model-only request unexpectedly advertises tools".to_owned(),
                ));
            }
            StoredOperationState::ModelPending {
                progress: OperationProgress {
                    runtime: model_only_runtime(
                        runtime_revision,
                        request.system_prompt,
                        max_model_attempts,
                    ),
                    model_attempts: attempt_count,
                },
                effect_id,
                settlement_token,
                assistant_entry_id,
                output_id,
            }
        }
        StoredOperationStateV1::Completed => StoredOperationState::Completed,
        StoredOperationStateV1::Failed => StoredOperationState::Failed {
            kind: FailureKind::General,
        },
    };
    serde_json::to_string(&StoredState::from_state(state)).map_err(json_error)
}

fn load_pending_v1_request(
    transaction: &Transaction<'_>,
    effect_id: Uuid,
) -> Result<ModelRequest, HarnessError> {
    let request_json = transaction
        .query_row(
            "SELECT request_json FROM model_attempts
             WHERE effect_id = ?1 AND status = 'pending'",
            [effect_id.to_string()],
            |row| row.get::<_, String>(0),
        )
        .optional()
        .map_err(sqlite_error)?
        .ok_or_else(|| HarnessError::Corrupt("v1 pending model request is missing".to_owned()))?;
    serde_json::from_str(&request_json).map_err(json_error)
}

fn model_only_runtime(
    revision: String,
    system_prompt: String,
    max_model_attempts: u32,
) -> FrozenRuntime {
    FrozenRuntime {
        revision,
        system_prompt,
        max_model_attempts,
        max_tool_calls_per_step: 0,
        tools: Vec::new(),
    }
}

#[derive(Deserialize)]
struct StoredStateV1 {
    format_version: u32,
    state: StoredOperationStateV1,
}

#[derive(Deserialize)]
struct StoredStateVersion {
    format_version: u32,
}

#[derive(Deserialize)]
#[serde(tag = "phase", rename_all = "snake_case")]
enum StoredOperationStateV1 {
    Queued,
    NeedModel {
        runtime_revision: String,
        system_prompt: String,
        max_model_attempts: u32,
        attempt_count: u32,
    },
    ModelPending {
        runtime_revision: String,
        max_model_attempts: u32,
        attempt_count: u32,
        effect_id: Uuid,
        settlement_token: Uuid,
        assistant_entry_id: Uuid,
        output_id: Uuid,
    },
    Completed,
    Failed,
}

pub(crate) fn open_connection(path: &Path) -> Result<Connection, HarnessError> {
    let connection = Connection::open(path).map_err(sqlite_error)?;
    connection
        .execute_batch(
            "PRAGMA foreign_keys = ON;
             PRAGMA journal_mode = WAL;
             PRAGMA synchronous = FULL;
             PRAGMA busy_timeout = 5000;",
        )
        .map_err(sqlite_error)?;
    Ok(connection)
}

#[allow(
    clippy::needless_pass_by_value,
    reason = "the owned signature is required by Result::map_err"
)]
pub(crate) fn sqlite_error(error: rusqlite::Error) -> HarnessError {
    HarnessError::Store(format!("SQLite error: {error}"))
}

#[allow(
    clippy::needless_pass_by_value,
    reason = "the owned signature is required by Result::map_err"
)]
pub(crate) fn json_error(error: serde_json::Error) -> HarnessError {
    HarnessError::Corrupt(format!("invalid JSON: {error}"))
}

#[cfg(test)]
mod tests {
    use tempfile::tempdir;

    use super::{initialize, open_connection};

    #[test]
    fn actual_connections_use_the_required_durability_settings() {
        let directory = tempdir().expect("temporary directory");
        let database = directory.path().join("harness.sqlite3");
        let mut connection = open_connection(&database).expect("open connection");
        initialize(&mut connection).expect("initialize schema");

        assert_eq!(pragma_integer(&connection, "foreign_keys"), 1);
        assert_eq!(pragma_text(&connection, "journal_mode"), "wal");
        assert_eq!(pragma_integer(&connection, "synchronous"), 2);
        assert_eq!(pragma_integer(&connection, "busy_timeout"), 5_000);
        assert_eq!(pragma_integer(&connection, "user_version"), 4);
    }

    fn pragma_integer(connection: &rusqlite::Connection, name: &str) -> i64 {
        connection
            .pragma_query_value(None, name, |row| row.get(0))
            .expect("integer pragma")
    }

    fn pragma_text(connection: &rusqlite::Connection, name: &str) -> String {
        connection
            .pragma_query_value(None, name, |row| row.get(0))
            .expect("text pragma")
    }
}
