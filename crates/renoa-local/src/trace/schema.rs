use std::path::Path;

use renoa_kernel::{AgentId, SessionId};
use rusqlite::{Connection, params};

use super::TraceError;

const SCHEMA_VERSION: i64 = 4;

pub(super) fn create(
    path: &Path,
    session_id: SessionId,
    agent_id: AgentId,
) -> Result<(), TraceError> {
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
            session_id TEXT NOT NULL,
            agent_id TEXT NOT NULL
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
        "INSERT INTO trace_metadata(schema_version, session_id, agent_id)
         VALUES (?1, ?2, ?3)",
        params![SCHEMA_VERSION, session_id.to_string(), agent_id.to_string()],
    )?;
    connection.execute_batch("PRAGMA wal_checkpoint(TRUNCATE);")?;
    Ok(())
}

pub(super) fn open(
    path: &Path,
    session_id: SessionId,
    agent_id: AgentId,
) -> Result<Connection, TraceError> {
    if !path.is_file() {
        return Err(TraceError::Incompatible(format!(
            "{} is not an existing trace database",
            path.display()
        )));
    }
    let connection = Connection::open(path)?;
    configure(&connection)?;
    verify_identity(&connection, session_id, agent_id)?;
    match stored_version(&connection)? {
        SCHEMA_VERSION => {}
        3 => strip_content(&connection, path)?,
        version => {
            return Err(TraceError::Incompatible(format!(
                "schema version {version} is unsupported; expected {SCHEMA_VERSION}"
            )));
        }
    }
    Ok(connection)
}

/// Schema 4 keeps no content. A schema 3 trace loses its streamed pieces and
/// tool progress rows and every request, response, argument and output
/// payload, then gives the space back. Failed tool errors are not kept either:
/// their earlier rows hold the whole output, not an excerpt.
fn strip_content(connection: &Connection, path: &Path) -> Result<(), TraceError> {
    let before = stored_bytes(path);
    let transaction = connection.unchecked_transaction()?;
    let removed = transaction.execute(
        "DELETE FROM events WHERE kind IN ('chunk', 'stream_chunk', 'execution_update')",
        [],
    )?;
    let cleared = transaction.execute(
        "UPDATE events SET payload_json = 'null'
         WHERE kind IN ('request_started', 'provider_request', 'request_finished',
                        'execution_started', 'execution_finished')",
        [],
    )?;
    transaction.execute("UPDATE runs SET input_json = 'null'", [])?;
    transaction.execute(
        "UPDATE trace_metadata SET schema_version = ?1",
        [SCHEMA_VERSION],
    )?;
    transaction.commit()?;
    connection.execute_batch("VACUUM; PRAGMA wal_checkpoint(TRUNCATE);")?;
    renoa_telemetry::event(
        "renoa.host",
        "info",
        "trace_content_removed",
        &serde_json::json!({
            "trace": path.display().to_string(),
            "rows_removed": removed,
            "payloads_cleared": cleared,
            "bytes_before": before,
            "bytes_after": stored_bytes(path),
        }),
    );
    Ok(())
}

/// The database and its write-ahead log together.
fn stored_bytes(path: &Path) -> u64 {
    let wal = path.with_extension("sqlite3-wal");
    [path, wal.as_path()]
        .iter()
        .filter_map(|file| std::fs::metadata(file).ok())
        .map(|metadata| metadata.len())
        .sum()
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

fn stored_version(connection: &Connection) -> Result<i64, TraceError> {
    Ok(
        connection.query_row("SELECT schema_version FROM trace_metadata", [], |row| {
            row.get(0)
        })?,
    )
}

fn verify_identity(
    connection: &Connection,
    session_id: SessionId,
    agent_id: AgentId,
) -> Result<(), TraceError> {
    require_one_metadata_row(connection)?;
    let (stored_session, stored_agent) = connection.query_row(
        "SELECT session_id, agent_id FROM trace_metadata",
        [],
        |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
    )?;
    if stored_session != session_id.to_string() {
        return Err(TraceError::Incompatible(
            "session identity does not match its trace database".to_owned(),
        ));
    }
    if stored_agent != agent_id.to_string() {
        return Err(TraceError::Incompatible(
            "agent identity does not match its trace database".to_owned(),
        ));
    }
    Ok(())
}

fn require_one_metadata_row(connection: &Connection) -> Result<(), TraceError> {
    let rows = connection.query_row("SELECT count(*) FROM trace_metadata", [], |row| {
        row.get::<_, i64>(0)
    })?;
    if rows == 1 {
        Ok(())
    } else {
        Err(TraceError::Incompatible(format!(
            "metadata must contain exactly one row; found {rows}"
        )))
    }
}
