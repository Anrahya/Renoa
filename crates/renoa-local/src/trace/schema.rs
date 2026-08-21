use std::path::Path;

use renoa_kernel::SessionId;
use rusqlite::{Connection, OptionalExtension as _, params};

use super::TraceError;

const SCHEMA_VERSION: i64 = 1;

pub(super) fn create(path: &Path, session_id: SessionId) -> Result<(), TraceError> {
    if path.exists() {
        return Err(TraceError::Incompatible(format!(
            "{} already exists",
            path.display()
        )));
    }
    let connection = Connection::open(path)?;
    configure(&connection)?;
    connection.execute_batch(
        "
        CREATE TABLE trace_metadata (
            schema_version INTEGER PRIMARY KEY,
            session_id TEXT NOT NULL
        ) STRICT;

        CREATE TABLE runs (
            run_id TEXT PRIMARY KEY,
            session_id TEXT NOT NULL,
            command_id TEXT NOT NULL,
            started_at_ms INTEGER NOT NULL,
            finished_at_ms INTEGER,
            duration_us INTEGER,
            status TEXT NOT NULL CHECK (status IN ('running', 'completed', 'cancelled', 'failed', 'waiting_for_input', 'interrupted')),
            trace_complete INTEGER NOT NULL CHECK (trace_complete IN (0, 1)),
            provider TEXT NOT NULL,
            model TEXT NOT NULL,
            reasoning TEXT NOT NULL,
            input_json TEXT NOT NULL CHECK (json_valid(input_json)),
            error_code TEXT,
            error_message TEXT
        ) STRICT;

        CREATE TABLE events (
            run_id TEXT NOT NULL,
            sequence INTEGER NOT NULL CHECK (sequence > 0),
            occurred_at_ms INTEGER NOT NULL,
            elapsed_us INTEGER NOT NULL CHECK (elapsed_us >= 0),
            duration_us INTEGER,
            time_to_first_output_us INTEGER,
            component TEXT NOT NULL,
            kind TEXT NOT NULL,
            correlation_id TEXT,
            name TEXT,
            status TEXT,
            input_tokens INTEGER,
            output_tokens INTEGER,
            cache_read_tokens INTEGER,
            cache_write_tokens INTEGER,
            payload_json TEXT NOT NULL CHECK (json_valid(payload_json)),
            PRIMARY KEY (run_id, sequence),
            FOREIGN KEY (run_id) REFERENCES runs(run_id) ON DELETE CASCADE
        ) STRICT;

        CREATE INDEX events_component_kind ON events(component, kind);
        CREATE INDEX events_correlation ON events(run_id, correlation_id);
        ",
    )?;
    connection.execute(
        "INSERT INTO trace_metadata(schema_version, session_id) VALUES (?1, ?2)",
        params![SCHEMA_VERSION, session_id.to_string()],
    )?;
    connection.execute_batch("PRAGMA wal_checkpoint(TRUNCATE);")?;
    Ok(())
}

pub(super) fn open(path: &Path, session_id: SessionId) -> Result<Connection, TraceError> {
    if !path.is_file() {
        return Err(TraceError::Incompatible(format!(
            "{} is not an existing trace database",
            path.display()
        )));
    }
    let connection = Connection::open(path)?;
    configure(&connection)?;
    verify_connection(&connection, session_id)?;
    Ok(connection)
}

pub(super) fn recover_running(connection: &Connection) -> Result<(), TraceError> {
    let finished_at_ms = super::record::now_unix_ms();
    connection.execute(
        "UPDATE runs
         SET finished_at_ms = ?1,
             duration_us = MAX(0, (?1 - started_at_ms) * 1000),
             status = 'interrupted',
             trace_complete = 0,
             error_code = COALESCE(error_code, 'trace_owner_interrupted'),
             error_message = COALESCE(
                 error_message,
                 'trace owner ended before finalizing the run'
             )
         WHERE status = 'running'",
        [finished_at_ms],
    )?;
    Ok(())
}

fn configure(connection: &Connection) -> Result<(), rusqlite::Error> {
    connection.busy_timeout(std::time::Duration::from_secs(5))?;
    connection.execute_batch(
        "PRAGMA foreign_keys = ON;
         PRAGMA journal_mode = WAL;
         PRAGMA synchronous = NORMAL;",
    )?;
    Ok(())
}

fn verify_connection(connection: &Connection, session_id: SessionId) -> Result<(), TraceError> {
    let metadata = connection
        .query_row(
            "SELECT schema_version, session_id FROM trace_metadata",
            [],
            |row| Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?)),
        )
        .optional()?;
    let Some((version, stored_session)) = metadata else {
        return Err(TraceError::Incompatible("metadata is missing".to_owned()));
    };
    if version != SCHEMA_VERSION {
        return Err(TraceError::Incompatible(format!(
            "schema version {version} is unsupported; expected {SCHEMA_VERSION}"
        )));
    }
    if stored_session != session_id.to_string() {
        return Err(TraceError::Incompatible(
            "session identity does not match its trace database".to_owned(),
        ));
    }
    Ok(())
}
