//! Catalog schema (schema 56) and SQLite queries for `host_apps`.

use std::{collections::HashSet, io, path::Path};

use rusqlite::{OptionalExtension as _, Transaction, TransactionBehavior, params};
use uuid::Uuid;

use super::{AppError, AppState, LiveApp};
use crate::host::catalog::{self, HostCatalogError};

const REACHABLE: &str = "(principal_id IS NULL OR principal_id = ?1)";

#[derive(Debug, Clone)]
pub(super) struct AppRecord {
    pub app: LiveApp,
    pub principal_id: Option<String>,
    pub app_id: String,
}

/// Schema 56: live apps supervised by this Host.
pub(crate) fn initialize(tx: &Transaction<'_>) -> Result<(), HostCatalogError> {
    tx.execute_batch(
        "CREATE TABLE IF NOT EXISTS host_apps (
            app_name TEXT PRIMARY KEY CHECK (length(app_name) >= 1 AND length(app_name) <= 63),
            agent_id TEXT NOT NULL REFERENCES host_agents(agent_id) ON DELETE CASCADE,
            principal_id TEXT CHECK (principal_id IS NULL OR length(principal_id) = 36),
            app_id TEXT NOT NULL UNIQUE CHECK (length(app_id) = 36),
            source_folder TEXT NOT NULL CHECK (length(source_folder) > 0),
            command TEXT NOT NULL CHECK (length(command) > 0),
            port INTEGER NOT NULL UNIQUE CHECK (port >= 20000 AND port <= 29999),
            state TEXT NOT NULL CHECK (state IN ('running', 'stopped', 'failed')),
            failure_reason TEXT,
            updated_at_ms INTEGER NOT NULL,
            last_active_ms INTEGER NOT NULL
        ) STRICT;",
    )?;
    Ok(())
}

/// Turns a schema 56 catalog back into schema 55's tables.
#[cfg(test)]
pub(crate) fn restore_schema_55(connection: &rusqlite::Connection) {
    connection
        .execute_batch("DROP TABLE host_apps;")
        .expect("restore schema 55");
}

pub(super) fn parse_state(raw: &str) -> Result<AppState, AppError> {
    match raw {
        "running" => Ok(AppState::Running),
        "stopped" => Ok(AppState::Stopped),
        "failed" => Ok(AppState::Failed),
        other => Err(AppError::Io(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("unknown app state `{other}`"),
        ))),
    }
}

pub(super) fn ensure_reachable(rec: &AppRecord, principal: Option<Uuid>) -> Result<(), AppError> {
    match (&rec.principal_id, principal) {
        (None, _) => Ok(()),
        (Some(owner), Some(caller)) if owner == &caller.to_string() => Ok(()),
        (Some(_), _) => Err(AppError::PermissionDenied),
    }
}

pub(super) fn used_ports(database: &Path) -> Result<HashSet<u16>, AppError> {
    let conn = catalog::open_read_only(database)?;
    let mut stmt = conn
        .prepare("SELECT port FROM host_apps")
        .map_err(HostCatalogError::from)?;
    let rows = stmt
        .query_map([], |row| row.get::<_, u16>(0))
        .map_err(HostCatalogError::from)?;
    let mut used = HashSet::new();
    for port in rows {
        used.insert(port.map_err(HostCatalogError::from)?);
    }
    Ok(used)
}

pub(super) fn insert_new_app(
    database: &Path,
    app: &LiveApp,
    principal_id: Option<&str>,
    app_id: &str,
) -> Result<(), AppError> {
    let mut conn = catalog::open_verified(database)?;
    let tx = conn
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(HostCatalogError::from)?;
    tx.execute(
        "INSERT INTO host_apps (
            app_name, agent_id, principal_id, app_id, source_folder, command,
            port, state, failure_reason, updated_at_ms, last_active_ms
         ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, 'running', NULL, ?8, ?9)",
        params![
            app.name,
            app.agent_id,
            principal_id,
            app_id,
            app.source_folder,
            app.command,
            app.port,
            app.updated_at_ms,
            app.last_active_ms,
        ],
    )
    .map_err(HostCatalogError::from)?;
    tx.commit().map_err(HostCatalogError::from)?;
    Ok(())
}

pub(super) fn update_existing_on_start(
    database: &Path,
    app_name: &str,
    agent_id: &str,
    principal_id: Option<&str>,
    source_folder: &str,
    command: &str,
    now_ms: i64,
) -> Result<(), AppError> {
    let conn = catalog::open_verified(database)?;
    let changed = conn
        .execute(
            "UPDATE host_apps SET
                agent_id = ?2,
                principal_id = CASE WHEN principal_id IS NULL THEN NULL ELSE COALESCE(?3, principal_id) END,
                source_folder = ?4,
                command = ?5,
                state = 'running',
                failure_reason = NULL,
                updated_at_ms = ?6,
                last_active_ms = ?6
             WHERE app_name = ?1",
            params![app_name, agent_id, principal_id, source_folder, command, now_ms],
        )
        .map_err(HostCatalogError::from)?;
    if changed == 0 {
        return Err(AppError::NotFound);
    }
    Ok(())
}

#[allow(
    clippy::too_many_arguments,
    reason = "columns of one host_apps row update"
)]
pub(super) fn update_existing_on_failure(
    database: &Path,
    app_name: &str,
    agent_id: &str,
    principal_id: Option<&str>,
    source_folder: &str,
    command: &str,
    reason: &str,
    now_ms: i64,
) -> Result<(), AppError> {
    let conn = catalog::open_verified(database)?;
    let changed = conn
        .execute(
            "UPDATE host_apps SET
                agent_id = ?2,
                principal_id = CASE WHEN principal_id IS NULL THEN NULL ELSE COALESCE(?3, principal_id) END,
                source_folder = ?4,
                command = ?5,
                state = 'failed',
                failure_reason = ?6,
                updated_at_ms = ?7
             WHERE app_name = ?1",
            params![
                app_name,
                agent_id,
                principal_id,
                source_folder,
                command,
                reason,
                now_ms
            ],
        )
        .map_err(HostCatalogError::from)?;
    if changed == 0 {
        return Err(AppError::NotFound);
    }
    Ok(())
}

pub(super) fn set_state_running(
    database: &Path,
    app_name: &str,
    now_ms: i64,
) -> Result<(), AppError> {
    let conn = catalog::open_verified(database)?;
    conn.execute(
        "UPDATE host_apps SET state = 'running', failure_reason = NULL,
         updated_at_ms = ?2, last_active_ms = ?2 WHERE app_name = ?1",
        params![app_name, now_ms],
    )
    .map_err(HostCatalogError::from)?;
    Ok(())
}

pub(super) fn set_state_stopped(
    database: &Path,
    app_name: &str,
    now_ms: i64,
) -> Result<(), AppError> {
    let conn = catalog::open_verified(database)?;
    conn.execute(
        "UPDATE host_apps SET state = 'stopped', failure_reason = NULL, updated_at_ms = ?2
         WHERE app_name = ?1",
        params![app_name, now_ms],
    )
    .map_err(HostCatalogError::from)?;
    Ok(())
}

pub(super) fn update_last_active(
    database: &Path,
    app_name: &str,
    now_ms: i64,
) -> Result<(), AppError> {
    let conn = catalog::open_verified(database)?;
    conn.execute(
        "UPDATE host_apps SET last_active_ms = ?2 WHERE app_name = ?1 AND state = 'running'",
        params![app_name, now_ms],
    )
    .map_err(HostCatalogError::from)?;
    Ok(())
}

pub(super) fn set_state_failed(
    database: &Path,
    app_name: &str,
    reason: &str,
    now_ms: i64,
) -> Result<(), AppError> {
    let conn = catalog::open_verified(database)?;
    conn.execute(
        "UPDATE host_apps SET state = 'failed', failure_reason = ?2, updated_at_ms = ?3
         WHERE app_name = ?1",
        params![app_name, reason, now_ms],
    )
    .map_err(HostCatalogError::from)?;
    Ok(())
}

fn row_to_record(row: &rusqlite::Row<'_>) -> rusqlite::Result<AppRecord> {
    let state_raw: String = row.get(7)?;
    let state = parse_state(&state_raw).map_err(|err| {
        rusqlite::Error::FromSqlConversionFailure(7, rusqlite::types::Type::Text, Box::new(err))
    })?;
    Ok(AppRecord {
        app: LiveApp {
            name: row.get(0)?,
            agent_id: row.get(1)?,
            source_folder: row.get(4)?,
            command: row.get(5)?,
            port: row.get(6)?,
            state,
            failure_reason: row.get(8)?,
            updated_at_ms: row.get(9)?,
            last_active_ms: row.get(10)?,
        },
        principal_id: row.get(2)?,
        app_id: row.get(3)?,
    })
}

pub(super) fn read_record(database: &Path, app_name: &str) -> Result<Option<AppRecord>, AppError> {
    let conn = catalog::open_read_only(database)?;
    conn.query_row(
        "SELECT app_name, agent_id, principal_id, app_id, source_folder, command,
                port, state, failure_reason, updated_at_ms, last_active_ms
         FROM host_apps WHERE app_name = ?1",
        [app_name],
        row_to_record,
    )
    .optional()
    .map_err(|e| AppError::from(HostCatalogError::from(e)))
}

pub(super) fn list_records(
    database: &Path,
    principal: Option<Uuid>,
) -> Result<Vec<AppRecord>, AppError> {
    let conn = catalog::open_read_only(database)?;
    if let Some(caller) = principal {
        let sql = format!(
            "SELECT app_name, agent_id, principal_id, app_id, source_folder, command,
                    port, state, failure_reason, updated_at_ms, last_active_ms
             FROM host_apps WHERE {REACHABLE} ORDER BY app_name ASC"
        );
        let mut stmt = conn.prepare(&sql).map_err(HostCatalogError::from)?;
        let rows = stmt
            .query_map([caller.to_string()], row_to_record)
            .map_err(HostCatalogError::from)?;
        rows.collect::<Result<Vec<_>, _>>()
            .map_err(|e| AppError::from(HostCatalogError::from(e)))
    } else {
        let mut stmt = conn
            .prepare(
                "SELECT app_name, agent_id, principal_id, app_id, source_folder, command,
                        port, state, failure_reason, updated_at_ms, last_active_ms
                 FROM host_apps WHERE principal_id IS NULL ORDER BY app_name ASC",
            )
            .map_err(HostCatalogError::from)?;
        let rows = stmt
            .query_map([], row_to_record)
            .map_err(HostCatalogError::from)?;
        rows.collect::<Result<Vec<_>, _>>()
            .map_err(|e| AppError::from(HostCatalogError::from(e)))
    }
}

pub(super) fn list_all_records(database: &Path) -> Result<Vec<AppRecord>, AppError> {
    let conn = catalog::open_read_only(database)?;
    let mut stmt = conn
        .prepare(
            "SELECT app_name, agent_id, principal_id, app_id, source_folder, command,
                    port, state, failure_reason, updated_at_ms, last_active_ms
             FROM host_apps ORDER BY app_name ASC",
        )
        .map_err(HostCatalogError::from)?;
    let rows = stmt
        .query_map([], row_to_record)
        .map_err(HostCatalogError::from)?;
    rows.collect::<Result<Vec<_>, _>>()
        .map_err(|e| AppError::from(HostCatalogError::from(e)))
}
