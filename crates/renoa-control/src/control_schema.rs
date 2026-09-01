use std::path::Path;

use rusqlite::Connection;

use crate::{
    ControlError,
    control_migrations::{
        add_execution_command_causation, migrate_v3_execution_events, remove_harness_configuration,
    },
};

const SCHEMA_VERSION: i64 = 7;

pub(crate) fn initialize(connection: &mut Connection) -> Result<(), ControlError> {
    let version = connection
        .query_row("PRAGMA user_version", [], |row| row.get::<_, i64>(0))
        .map_err(sqlite_error)?;
    if version > SCHEMA_VERSION {
        return Err(ControlError::store(format!(
            "control database schema version {version} is newer than supported version {SCHEMA_VERSION}"
        )));
    }
    if legacy_tasks_lack_owner(connection)? {
        return Err(ControlError::store(
            "control database predates task ownership and cannot be opened safely",
        ));
    }
    if version == 3 {
        migrate_v3_execution_events(connection)?;
    }
    if (1..=4).contains(&version) {
        remove_harness_configuration(connection)?;
    }
    if (1..=5).contains(&version) {
        add_execution_command_causation(connection)?;
    }
    create_continuity_schema(connection)?;
    create_browser_identity_schema(connection)?;
    connection
        .execute_batch("PRAGMA user_version = 7;")
        .map_err(sqlite_error)
}

fn create_continuity_schema(connection: &Connection) -> Result<(), ControlError> {
    connection
        .execute_batch(
            "CREATE TABLE IF NOT EXISTS devices (
                device_id TEXT PRIMARY KEY,
                credential_hash BLOB NOT NULL UNIQUE CHECK(length(credential_hash) = 32),
                peer_json TEXT NOT NULL,
                revoked INTEGER NOT NULL DEFAULT 0 CHECK(revoked IN (0, 1))
            );

            CREATE TABLE IF NOT EXISTS enrollments (
                token_hash BLOB PRIMARY KEY CHECK(length(token_hash) = 32),
                peer_json TEXT NOT NULL,
                expires_at_ms INTEGER NOT NULL
            );

            CREATE TABLE IF NOT EXISTS tasks (
                task_id TEXT PRIMARY KEY,
                principal_id TEXT NOT NULL,
                node_id TEXT NOT NULL,
                target_json TEXT NOT NULL,
                next_sequence INTEGER NOT NULL
            );

            CREATE TABLE IF NOT EXISTS commands (
                command_id TEXT PRIMARY KEY,
                task_id TEXT NOT NULL REFERENCES tasks(task_id),
                command_json TEXT NOT NULL
            );

            CREATE TABLE IF NOT EXISTS task_events (
                event_id TEXT PRIMARY KEY,
                task_id TEXT NOT NULL REFERENCES tasks(task_id),
                sequence INTEGER NOT NULL,
                source_id TEXT NOT NULL UNIQUE,
                kind_json TEXT NOT NULL,
                UNIQUE(task_id, sequence)
            );

            CREATE TABLE IF NOT EXISTS pending_executions (
                command_id TEXT PRIMARY KEY REFERENCES commands(command_id) ON DELETE CASCADE,
                task_sequence INTEGER NOT NULL
            );

            CREATE TABLE IF NOT EXISTS execution_event_streams (
                command_id TEXT PRIMARY KEY REFERENCES commands(command_id) ON DELETE CASCADE,
                execution_id TEXT NOT NULL UNIQUE,
                next_sequence INTEGER NOT NULL CHECK(next_sequence >= 0),
                terminal INTEGER NOT NULL CHECK(terminal IN (0, 1))
            );

            CREATE INDEX IF NOT EXISTS task_events_task_sequence
                ON task_events(task_id, sequence);",
        )
        .map_err(sqlite_error)
}

fn create_browser_identity_schema(connection: &Connection) -> Result<(), ControlError> {
    connection
        .execute_batch(
            "CREATE TABLE IF NOT EXISTS passkey_bootstraps (
                token_hash BLOB PRIMARY KEY CHECK(length(token_hash) = 32),
                principal_id TEXT NOT NULL,
                expires_at_ms INTEGER NOT NULL
            );

            CREATE TABLE IF NOT EXISTS passkey_registration_ceremonies (
                ceremony_id TEXT PRIMARY KEY,
                principal_id TEXT NOT NULL,
                surface TEXT NOT NULL,
                state_json TEXT NOT NULL,
                expires_at_ms INTEGER NOT NULL
            );

            CREATE TABLE IF NOT EXISTS passkeys (
                credential_id BLOB PRIMARY KEY,
                principal_id TEXT NOT NULL,
                passkey_json TEXT NOT NULL,
                authentication_counter INTEGER NOT NULL DEFAULT 0
                    CHECK(authentication_counter >= 0),
                created_at_ms INTEGER NOT NULL
            );

            CREATE INDEX IF NOT EXISTS passkeys_principal
                ON passkeys(principal_id);

            CREATE TABLE IF NOT EXISTS passkey_authentication_ceremonies (
                ceremony_id TEXT PRIMARY KEY,
                principal_id TEXT NOT NULL,
                surface TEXT NOT NULL,
                state_json TEXT NOT NULL,
                expires_at_ms INTEGER NOT NULL
            );

            CREATE TABLE IF NOT EXISTS browser_connection_tickets (
                ticket_hash BLOB PRIMARY KEY CHECK(length(ticket_hash) = 32),
                principal_id TEXT NOT NULL,
                surface TEXT NOT NULL,
                expires_at_ms INTEGER NOT NULL
            );",
        )
        .map_err(sqlite_error)
}

fn legacy_tasks_lack_owner(connection: &Connection) -> Result<bool, ControlError> {
    let tasks_exist = connection
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = ?1)",
            ["tasks"],
            |row| row.get::<_, bool>(0),
        )
        .map_err(sqlite_error)?;
    if !tasks_exist {
        return Ok(false);
    }
    let mut statement = connection
        .prepare("PRAGMA table_info(tasks)")
        .map_err(sqlite_error)?;
    let names = statement
        .query_map([], |row| row.get::<_, String>(1))
        .map_err(sqlite_error)?;
    for name in names {
        if name.map_err(sqlite_error)? == "principal_id" {
            return Ok(false);
        }
    }
    Ok(true)
}

pub(crate) fn open_connection(path: &Path) -> Result<Connection, ControlError> {
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
fn sqlite_error(error: rusqlite::Error) -> ControlError {
    ControlError::store(format!("SQLite error: {error}"))
}
