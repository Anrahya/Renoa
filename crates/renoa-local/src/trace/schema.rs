use std::path::Path;

use renoa_kernel::{AgentId, SessionId};
use rusqlite::{Connection, TransactionBehavior, params};

use super::TraceError;
use crate::AgentProfileId;

const SCHEMA_VERSION: i64 = 2;

pub(super) fn create(
    path: &Path,
    session_id: SessionId,
    agent_id: AgentId,
    profile_id: &AgentProfileId,
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
            agent_id TEXT NOT NULL,
            profile_id TEXT NOT NULL
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
        "INSERT INTO trace_metadata(schema_version, session_id, agent_id, profile_id)
         VALUES (?1, ?2, ?3, ?4)",
        params![
            SCHEMA_VERSION,
            session_id.to_string(),
            agent_id.to_string(),
            profile_id.as_str()
        ],
    )?;
    connection.execute_batch("PRAGMA wal_checkpoint(TRUNCATE);")?;
    Ok(())
}

pub(super) fn open(
    path: &Path,
    session_id: SessionId,
    agent_id: AgentId,
    profile_id: &AgentProfileId,
) -> Result<Connection, TraceError> {
    if !path.is_file() {
        return Err(TraceError::Incompatible(format!(
            "{} is not an existing trace database",
            path.display()
        )));
    }
    let mut connection = Connection::open(path)?;
    configure(&connection)?;
    migrate(&mut connection, session_id, agent_id, profile_id)?;
    verify_connection(&connection, session_id, agent_id, profile_id)?;
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

fn migrate(
    connection: &mut Connection,
    session_id: SessionId,
    agent_id: AgentId,
    profile_id: &AgentProfileId,
) -> Result<(), TraceError> {
    require_one_metadata_row(connection)?;
    let version = connection.query_row("SELECT schema_version FROM trace_metadata", [], |row| {
        row.get::<_, i64>(0)
    })?;
    match version {
        SCHEMA_VERSION => Ok(()),
        1 => migrate_v1(connection, session_id, agent_id, profile_id),
        _ => Err(TraceError::Incompatible(format!(
            "schema version {version} is unsupported; expected {SCHEMA_VERSION}"
        ))),
    }
}

fn migrate_v1(
    connection: &mut Connection,
    session_id: SessionId,
    agent_id: AgentId,
    profile_id: &AgentProfileId,
) -> Result<(), TraceError> {
    let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let stored_session = transaction.query_row(
        "SELECT session_id FROM trace_metadata WHERE schema_version = 1",
        [],
        |row| row.get::<_, String>(0),
    )?;
    if stored_session != session_id.to_string() {
        return Err(TraceError::Incompatible(
            "session identity does not match its trace database".to_owned(),
        ));
    }
    transaction.execute_batch(
        "ALTER TABLE trace_metadata RENAME TO trace_metadata_v1;
         CREATE TABLE trace_metadata (
             schema_version INTEGER PRIMARY KEY,
             session_id TEXT NOT NULL,
             agent_id TEXT NOT NULL,
             profile_id TEXT NOT NULL
         ) STRICT;",
    )?;
    transaction.execute(
        "INSERT INTO trace_metadata(schema_version, session_id, agent_id, profile_id)
         VALUES (?1, ?2, ?3, ?4)",
        params![
            SCHEMA_VERSION,
            session_id.to_string(),
            agent_id.to_string(),
            profile_id.as_str()
        ],
    )?;
    transaction.execute_batch("DROP TABLE trace_metadata_v1;")?;
    transaction.commit()?;
    Ok(())
}

fn verify_connection(
    connection: &Connection,
    session_id: SessionId,
    agent_id: AgentId,
    profile_id: &AgentProfileId,
) -> Result<(), TraceError> {
    require_one_metadata_row(connection)?;
    let (version, stored_session, stored_agent, stored_profile) = connection.query_row(
        "SELECT schema_version, session_id, agent_id, profile_id FROM trace_metadata",
        [],
        |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
            ))
        },
    )?;
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
    if stored_agent != agent_id.to_string() {
        return Err(TraceError::Incompatible(
            "agent identity does not match its trace database".to_owned(),
        ));
    }
    if stored_profile != profile_id.as_str() {
        return Err(TraceError::Incompatible(
            "profile identity does not match its trace database".to_owned(),
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
