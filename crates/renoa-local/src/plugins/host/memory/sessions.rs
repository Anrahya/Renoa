//! Which memory session an agent's jobs run in. The session of generation `g`
//! has an id derived from the agent and `g`; an agent starts at generation 0,
//! and each move to the next is recorded with why, when a session could not
//! go on. Earlier sessions are kept and stay readable.

use std::path::Path;

use renoa_kernel::AgentId;
use rusqlite::{Connection, TransactionBehavior, params};
use uuid::Uuid;

use super::jobs::{self, Receipt};
use crate::host::catalog::{self, HostCatalogError};

/// One row per move: the generation an agent moved to, why, and when.
pub(crate) const SESSIONS_TABLE: &str = "CREATE TABLE IF NOT EXISTS host_memory_sessions (
        agent_id TEXT NOT NULL REFERENCES host_agents(agent_id),
        generation INTEGER NOT NULL CHECK (generation > 0),
        reason TEXT NOT NULL,
        moved_at_ms INTEGER NOT NULL,
        PRIMARY KEY (agent_id, generation)
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
    u32::try_from(generation)
        .map_err(|error| HostCatalogError::Invalid(format!("memory session generation: {error}")))
}

/// Moves `agent` to its next generation because of `reason`, and returns it.
/// The job of `abandoning`, which its session could not run, is closed as
/// abandoned in the same transaction.
pub(crate) fn move_on(
    database: &Path,
    agent: AgentId,
    abandoning: Option<Uuid>,
    reason: &str,
    now_ms: i64,
) -> Result<u32, HostCatalogError> {
    let mut connection = catalog::open_verified(database)?;
    let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
    if let Some(command) = abandoning {
        jobs::settle_in(&transaction, command, Receipt::Abandoned)?;
    }
    let generation = current(&transaction, agent)?
        .checked_add(1)
        .ok_or_else(|| {
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
