use std::path::Path;

use rusqlite::{Connection, TransactionBehavior};

use crate::HarnessError;

const SCHEMA_VERSION: u32 = 1;

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
    transaction
        .execute_batch(
            "CREATE TABLE sessions (
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
             ) STRICT;",
        )
        .map_err(sqlite_error)?;
    transaction
        .pragma_update(None, "user_version", SCHEMA_VERSION)
        .map_err(sqlite_error)?;
    transaction.commit().map_err(sqlite_error)
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
        assert_eq!(pragma_integer(&connection, "user_version"), 1);
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
