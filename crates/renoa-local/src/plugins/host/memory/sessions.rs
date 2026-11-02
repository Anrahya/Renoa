//! Which memory session an agent's jobs run in. The session of generation `g`
//! has an id derived from the agent and `g`; an agent starts at generation 0,
//! and each move to the next is recorded with why, when a session could not
//! go on. Earlier sessions are kept and stay readable. A generation also
//! counts the distinct runs that could not open its session.

use std::path::Path;

use renoa_kernel::AgentId;
use rusqlite::{Connection, TransactionBehavior, params};
use uuid::Uuid;

use super::jobs::{self, Receipt};
use crate::host::catalog::{self, HostCatalogError};

/// One row per generation an agent moved to, with why and when, and per
/// generation whose session failed to open, the runs that failed it. Only
/// generation 0 is never moved to.
pub(crate) const SESSIONS_TABLE: &str = "CREATE TABLE IF NOT EXISTS host_memory_sessions (
        agent_id TEXT NOT NULL REFERENCES host_agents(agent_id),
        generation INTEGER NOT NULL CHECK (generation >= 0),
        reason TEXT,
        moved_at_ms INTEGER,
        open_failures INTEGER NOT NULL DEFAULT 0 CHECK (open_failures >= 0),
        last_failed_run TEXT,
        PRIMARY KEY (agent_id, generation),
        CHECK ((generation = 0) = (reason IS NULL)),
        CHECK ((reason IS NULL) = (moved_at_ms IS NULL))
    ) STRICT;";

pub(crate) fn initialize(transaction: &rusqlite::Transaction<'_>) -> rusqlite::Result<()> {
    transaction.execute_batch(SESSIONS_TABLE)
}

/// The memory session of `agent` at `generation`, apart from its
/// conversations.
pub(crate) fn session_id(agent: AgentId, generation: u32) -> Uuid {
    crate::stable_id::stable_id(&format!("renoa.memory.session.v1:{agent}:{generation}"))
}

/// The generation `agent`'s jobs run in now.
pub(crate) fn current(db: &Connection, agent: AgentId) -> Result<u32, HostCatalogError> {
    let generation: i64 = db.query_row(
        "SELECT COALESCE(MAX(generation), 0) FROM host_memory_sessions WHERE agent_id = ?1",
        [agent.to_string()],
        |row| row.get(0),
    )?;
    count(generation)
}

/// Counts `run` among the runs that could not open the session of
/// `generation`, once however often it tried, and returns how many have.
/// Generation 0 gets its row here; a later one has had its row since the
/// agent moved to it.
pub(crate) fn failed_open(
    database: &Path,
    agent: AgentId,
    generation: u32,
    run: Uuid,
) -> Result<u32, HostCatalogError> {
    let mut connection = catalog::open_verified(database)?;
    let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
    if generation == 0 {
        transaction.execute(
            "INSERT OR IGNORE INTO host_memory_sessions(agent_id, generation) VALUES (?1, 0)",
            [agent.to_string()],
        )?;
    }
    let failures: i64 = transaction.query_row(
        "UPDATE host_memory_sessions SET
            open_failures = open_failures + (last_failed_run IS NOT ?3),
            last_failed_run = ?3
         WHERE agent_id = ?1 AND generation = ?2
         RETURNING open_failures",
        params![agent.to_string(), generation, run.to_string()],
        |row| row.get(0),
    )?;
    transaction.commit()?;
    count(failures)
}

/// Forgets the open failures of `generation`, whose session opened.
pub(crate) fn opened(
    database: &Path,
    agent: AgentId,
    generation: u32,
) -> Result<(), HostCatalogError> {
    catalog::open_verified(database)?.execute(
        "UPDATE host_memory_sessions SET open_failures = 0, last_failed_run = NULL
         WHERE agent_id = ?1 AND generation = ?2 AND open_failures > 0",
        params![agent.to_string(), generation],
    )?;
    Ok(())
}

/// Moves `agent` from `observed`, the generation whose session could not go
/// on, to the next, because of `reason`, and returns it. The job of
/// `abandoning` is closed as abandoned in the same transaction. Nothing
/// changes when the agent moved past `observed` meanwhile.
pub(crate) fn move_on(
    database: &Path,
    agent: AgentId,
    observed: u32,
    abandoning: Option<Uuid>,
    reason: &str,
    now_ms: i64,
) -> Result<u32, HostCatalogError> {
    let mut connection = catalog::open_verified(database)?;
    let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
    if let Some(command) = abandoning {
        jobs::settle_in(&transaction, command, Receipt::Abandoned)?;
    }
    let found = current(&transaction, agent)?;
    if found != observed {
        return Err(HostCatalogError::Invalid(format!(
            "the memory session moved to generation {found} meanwhile"
        )));
    }
    let generation = observed.checked_add(1).ok_or_else(|| {
        HostCatalogError::Invalid("memory session generations ran out".to_owned())
    })?;
    transaction.execute(
        "INSERT INTO host_memory_sessions(agent_id, generation, reason, moved_at_ms)
         VALUES (?1, ?2, ?3, ?4)",
        params![agent.to_string(), generation, reason, now_ms],
    )?;
    transaction.commit()?;
    Ok(generation)
}

/// The reason `agent` moved to `generation`, if it did.
#[cfg(test)]
pub(crate) fn reason(
    db: &Connection,
    agent: AgentId,
    generation: u32,
) -> Result<Option<String>, HostCatalogError> {
    use rusqlite::OptionalExtension as _;

    Ok(db
        .query_row(
            "SELECT reason FROM host_memory_sessions WHERE agent_id = ?1 AND generation = ?2",
            params![agent.to_string(), generation],
            |row| row.get(0),
        )
        .optional()?
        .flatten())
}

fn count(stored: i64) -> Result<u32, HostCatalogError> {
    u32::try_from(stored)
        .map_err(|error| HostCatalogError::Invalid(format!("memory session count: {error}")))
}
