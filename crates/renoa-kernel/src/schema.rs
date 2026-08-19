use std::path::Path;

use rusqlite::{Connection, OptionalExtension, TransactionBehavior};

use crate::{KernelError, StoreError};

const SCHEMA_VERSION: u32 = 2;
pub(crate) const OPERATION_STATE_VERSION: u32 = 1;

const SCHEMA: &str = "CREATE TABLE agents (
        agent_id TEXT PRIMARY KEY NOT NULL
     ) STRICT;

     CREATE TABLE sessions (
        session_id TEXT PRIMARY KEY NOT NULL,
        agent_id TEXT NOT NULL REFERENCES agents(agent_id),
        next_command_position INTEGER NOT NULL CHECK (next_command_position >= 0),
        active_operation_id TEXT,
        next_event_sequence INTEGER NOT NULL CHECK (next_event_sequence >= 0),
        FOREIGN KEY (session_id, active_operation_id)
            REFERENCES operations(session_id, operation_id)
            DEFERRABLE INITIALLY DEFERRED
     ) STRICT;

     CREATE TABLE commands (
        command_id TEXT PRIMARY KEY NOT NULL,
        session_id TEXT NOT NULL REFERENCES sessions(session_id),
        content_json TEXT NOT NULL,
        UNIQUE (session_id, command_id)
     ) STRICT;

     CREATE TABLE operations (
        operation_id TEXT PRIMARY KEY NOT NULL,
        session_id TEXT NOT NULL REFERENCES sessions(session_id),
        command_id TEXT NOT NULL UNIQUE,
        position INTEGER NOT NULL CHECK (position >= 0),
        phase TEXT NOT NULL CHECK (
            phase IN (
                'queued', 'need_decision', 'effect_intent',
                'effect_dispatched', 'outcome_unknown', 'waiting',
                'completed', 'failed', 'cancelled'
            )
        ),
        state_version INTEGER NOT NULL CHECK (state_version > 0),
        transition_version INTEGER NOT NULL CHECK (transition_version >= 0),
        manifest_json TEXT,
        checkpoint_json TEXT,
        current_effect_id TEXT,
        input_effect_id TEXT,
        outcome_json TEXT,
        next_effect_position INTEGER NOT NULL CHECK (next_effect_position >= 0),
        UNIQUE (session_id, position),
        UNIQUE (session_id, operation_id),
        FOREIGN KEY (session_id, command_id)
            REFERENCES commands(session_id, command_id)
     ) STRICT;

     CREATE TABLE semantic_events (
        event_id TEXT PRIMARY KEY NOT NULL,
        session_id TEXT NOT NULL REFERENCES sessions(session_id),
        operation_id TEXT NOT NULL,
        sequence INTEGER NOT NULL CHECK (sequence >= 0),
        kind TEXT NOT NULL CHECK (length(kind) > 0),
        payload_json TEXT NOT NULL,
        UNIQUE (session_id, sequence),
        FOREIGN KEY (session_id, operation_id)
            REFERENCES operations(session_id, operation_id)
     ) STRICT;

     CREATE TABLE effects (
        effect_id TEXT PRIMARY KEY NOT NULL,
        operation_id TEXT NOT NULL REFERENCES operations(operation_id),
        position INTEGER NOT NULL CHECK (position >= 0),
        binding TEXT NOT NULL CHECK (length(binding) > 0),
        binding_revision TEXT NOT NULL CHECK (length(binding_revision) > 0),
        recovery TEXT NOT NULL CHECK (
            recovery IN ('safe_to_replay', 'never_replay')
        ),
        request_json TEXT NOT NULL,
        status TEXT NOT NULL CHECK (
            status IN (
                'intent_committed', 'dispatch_started',
                'settled', 'outcome_unknown'
            )
        ),
        dispatch_count INTEGER NOT NULL CHECK (dispatch_count >= 0),
        outcome_json TEXT,
        UNIQUE (operation_id, position),
        CHECK ((status = 'settled') = (outcome_json IS NOT NULL))
     ) STRICT;

     CREATE TABLE cancellation_requests (
        cancellation_id TEXT PRIMARY KEY NOT NULL,
        session_id TEXT NOT NULL,
        operation_id TEXT NOT NULL,
        FOREIGN KEY (session_id, operation_id)
            REFERENCES operations(session_id, operation_id)
     ) STRICT;

     CREATE INDEX cancellation_requests_operation
        ON cancellation_requests(session_id, operation_id);";

const MIGRATE_V1_TO_V2: &str = "CREATE TABLE operations_v2 (
        operation_id TEXT PRIMARY KEY NOT NULL,
        session_id TEXT NOT NULL REFERENCES sessions(session_id),
        command_id TEXT NOT NULL UNIQUE,
        position INTEGER NOT NULL CHECK (position >= 0),
        phase TEXT NOT NULL CHECK (
            phase IN (
                'queued', 'need_decision', 'effect_intent',
                'effect_dispatched', 'outcome_unknown', 'waiting',
                'completed', 'failed', 'cancelled'
            )
        ),
        state_version INTEGER NOT NULL CHECK (state_version > 0),
        transition_version INTEGER NOT NULL CHECK (transition_version >= 0),
        manifest_json TEXT,
        checkpoint_json TEXT,
        current_effect_id TEXT,
        input_effect_id TEXT,
        outcome_json TEXT,
        next_effect_position INTEGER NOT NULL CHECK (next_effect_position >= 0),
        UNIQUE (session_id, position),
        UNIQUE (session_id, operation_id),
        FOREIGN KEY (session_id, command_id)
            REFERENCES commands(session_id, command_id)
     ) STRICT;

     INSERT INTO operations_v2 (
        operation_id, session_id, command_id, position, phase, state_version,
        transition_version, manifest_json, checkpoint_json, current_effect_id,
        input_effect_id, outcome_json, next_effect_position
     ) SELECT
        operation_id, session_id, command_id, position, phase, state_version,
        transition_version, manifest_json, checkpoint_json, current_effect_id,
        input_effect_id, outcome_json, next_effect_position
     FROM operations;

     DROP TABLE operations;
     ALTER TABLE operations_v2 RENAME TO operations;

     CREATE TABLE cancellation_requests (
        cancellation_id TEXT PRIMARY KEY NOT NULL,
        session_id TEXT NOT NULL,
        operation_id TEXT NOT NULL,
        FOREIGN KEY (session_id, operation_id)
            REFERENCES operations(session_id, operation_id)
     ) STRICT;

     CREATE INDEX cancellation_requests_operation
        ON cancellation_requests(session_id, operation_id);";

pub(crate) fn initialize(connection: &mut Connection) -> Result<(), KernelError> {
    let version = connection
        .pragma_query_value(None, "user_version", |row| row.get::<_, u32>(0))
        .map_err(sqlite_error)?;
    if version > SCHEMA_VERSION {
        return Err(KernelError::UnsupportedSchema {
            found: version,
            supported: SCHEMA_VERSION,
        });
    }
    if version == SCHEMA_VERSION {
        return validate_database(connection);
    }
    if version == 1 {
        validate_database(connection)?;
        migrate_v1_to_v2(connection)?;
        return validate_database(connection);
    }
    let transaction = connection
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(sqlite_error)?;
    transaction.execute_batch(SCHEMA).map_err(sqlite_error)?;
    transaction
        .pragma_update(None, "user_version", SCHEMA_VERSION)
        .map_err(sqlite_error)?;
    transaction.commit().map_err(sqlite_error)?;
    validate_database(connection)
}

fn migrate_v1_to_v2(connection: &mut Connection) -> Result<(), KernelError> {
    connection
        .pragma_update(None, "foreign_keys", false)
        .map_err(sqlite_error)?;
    let migration = (|| {
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(sqlite_error)?;
        transaction
            .execute_batch(MIGRATE_V1_TO_V2)
            .map_err(sqlite_error)?;
        transaction
            .pragma_update(None, "user_version", SCHEMA_VERSION)
            .map_err(sqlite_error)?;
        transaction.commit().map_err(sqlite_error)
    })();
    let foreign_keys = connection
        .pragma_update(None, "foreign_keys", true)
        .map_err(sqlite_error);
    migration?;
    foreign_keys
}

fn validate_database(connection: &Connection) -> Result<(), KernelError> {
    validate_operation_versions(connection)?;
    validate_foreign_keys(connection)
}

fn validate_operation_versions(connection: &Connection) -> Result<(), KernelError> {
    let unsupported = connection
        .query_row(
            "SELECT state_version FROM operations
             WHERE state_version != ?1 LIMIT 1",
            [i64::from(OPERATION_STATE_VERSION)],
            |row| row.get::<_, i64>(0),
        )
        .optional()
        .map_err(sqlite_error)?;
    match unsupported {
        None => Ok(()),
        Some(found) if found > i64::from(OPERATION_STATE_VERSION) => {
            Err(KernelError::UnsupportedStateVersion {
                found: u32::try_from(found).unwrap_or(u32::MAX),
                supported: OPERATION_STATE_VERSION,
            })
        }
        Some(found) => Err(KernelError::Corrupt(format!(
            "invalid operation state version {found}"
        ))),
    }
}

fn validate_foreign_keys(connection: &Connection) -> Result<(), KernelError> {
    let mut statement = connection
        .prepare("PRAGMA foreign_key_check")
        .map_err(sqlite_error)?;
    let violation = statement
        .query_row([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, Option<i64>>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, i64>(3)?,
            ))
        })
        .optional()
        .map_err(sqlite_error)?;
    match violation {
        None => Ok(()),
        Some((table, row_id, parent, foreign_key)) => Err(KernelError::Corrupt(format!(
            "foreign-key violation in table `{table}` row {row_id:?} against `{parent}` constraint {foreign_key}"
        ))),
    }
}

pub(crate) fn open_connection(path: &Path) -> Result<Connection, KernelError> {
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
pub(crate) fn sqlite_error(error: rusqlite::Error) -> KernelError {
    KernelError::Store(StoreError::sqlite(error))
}

#[allow(
    clippy::needless_pass_by_value,
    reason = "the owned signature is required by Result::map_err"
)]
pub(crate) fn json_error(error: serde_json::Error) -> KernelError {
    KernelError::Corrupt(format!("invalid JSON: {error}"))
}

#[cfg(test)]
mod tests {
    use tempfile::tempdir;

    use super::{initialize, open_connection};
    use crate::{AgentId, Command, CommandId, Kernel, SessionId};

    #[test]
    fn actual_connections_use_required_durability_settings() {
        let directory = tempdir().expect("temporary directory");
        let database = directory.path().join("kernel.sqlite3");
        let mut connection = open_connection(&database).expect("open connection");
        initialize(&mut connection).expect("initialize schema");

        assert_eq!(pragma_integer(&connection, "foreign_keys"), 1);
        assert_eq!(pragma_text(&connection, "journal_mode"), "wal");
        assert_eq!(pragma_integer(&connection, "synchronous"), 2);
        assert_eq!(pragma_integer(&connection, "busy_timeout"), 5_000);
        assert_eq!(pragma_integer(&connection, "user_version"), 2);
    }

    #[test]
    fn version_one_data_migrates_with_its_cross_record_ownership_intact() {
        let directory = tempdir().expect("temporary directory");
        let database = directory.path().join("kernel.sqlite3");
        let kernel = Kernel::open(&database).expect("create current database");
        let agent_id = AgentId::new();
        let session_id = SessionId::new();
        kernel.create_agent(agent_id).expect("create agent");
        kernel
            .create_session(session_id, agent_id)
            .expect("create session");
        let admission = kernel
            .submit(
                session_id,
                Command::new(CommandId::new(), serde_json::json!({ "input": "saved" })),
            )
            .expect("submit command");
        drop(kernel);

        let connection = open_connection(&database).expect("open fixture database");
        connection
            .execute_batch(
                "DROP TABLE cancellation_requests;
                 PRAGMA user_version = 1;",
            )
            .expect("represent version one schema");
        drop(connection);

        let migrated = Kernel::open(&database).expect("migrate version one database");
        let snapshot = migrated.inspect(session_id).expect("inspect migrated data");
        assert_eq!(snapshot.operations[0].operation_id, admission.operation_id);
        drop(migrated);

        let connection = open_connection(&database).expect("inspect migration");
        assert_eq!(pragma_integer(&connection, "user_version"), 2);
        let cancellation_table: i64 = connection
            .query_row(
                "SELECT COUNT(*) FROM sqlite_schema
                 WHERE type = 'table' AND name = 'cancellation_requests'",
                [],
                |row| row.get(0),
            )
            .expect("inspect cancellation table");
        assert_eq!(cancellation_table, 1);
        let violations: i64 = connection
            .query_row("SELECT COUNT(*) FROM pragma_foreign_key_check", [], |row| {
                row.get(0)
            })
            .expect("check migrated foreign keys");
        assert_eq!(violations, 0);
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
