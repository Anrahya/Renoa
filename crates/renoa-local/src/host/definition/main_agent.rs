//! The Host's main agent: one per Host, not per person.
//!
//! The marker names the first agent made on the Host until the owner names
//! another through [`HostMainAgentControl`]. Its foreign key restricts deleting
//! the agent it names, so it can never point at a missing agent; a bulk reset
//! clears it with the agent rows. Being the main agent grants no authority.

use std::path::{Path, PathBuf};

use renoa_kernel::AgentId;
use rusqlite::{Connection, OptionalExtension as _, Transaction, TransactionBehavior, params};
use serde::{Deserialize, Serialize};
use thiserror::Error;
use uuid::Uuid;

use super::store;
use crate::{
    HostObserver, LocalHostError,
    host::catalog::{self, HostCatalogError},
};

/// The marker and the owner's receipts for changing it.
pub(in crate::host) fn initialize(transaction: &Transaction<'_>) -> Result<(), HostCatalogError> {
    transaction.execute_batch(
        "CREATE TABLE IF NOT EXISTS host_main_agent (
            singleton INTEGER PRIMARY KEY CHECK (singleton = 1),
            agent_id TEXT NOT NULL REFERENCES host_agents(agent_id) ON DELETE RESTRICT,
            revision INTEGER NOT NULL CHECK (revision > 0)
        ) STRICT;

        CREATE TABLE IF NOT EXISTS host_main_agent_operations (
            operation_id TEXT PRIMARY KEY,
            principal_id TEXT NOT NULL,
            request_json TEXT NOT NULL CHECK (json_valid(request_json)),
            result_json TEXT NOT NULL CHECK (json_valid(result_json))
        ) STRICT;",
    )?;
    Ok(())
}

/// Marks the earliest-created agent, ties broken by id, when no agent is
/// marked. Creation calls it inside its own transaction, where the new agent
/// is the only one an unmarked Host can hold; the schema 42 upgrade calls it to
/// mark the agent an earlier runtime made first.
pub(in crate::host) fn mark_first_if_unmarked(
    transaction: &Transaction<'_>,
) -> Result<(), HostCatalogError> {
    transaction.execute(
        "INSERT INTO host_main_agent(singleton, agent_id, revision)
         SELECT 1, agent_id, 1 FROM host_agents
         WHERE NOT EXISTS (SELECT 1 FROM host_main_agent)
         ORDER BY created_at_ms, agent_id
         LIMIT 1",
        [],
    )?;
    Ok(())
}

/// The Host's main agent and the revision a change must name.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct MainAgent {
    pub main_agent_id: Uuid,
    pub revision: i64,
}

/// Reads the marker; `None` only while the Host has no agent.
pub(in crate::host) fn read(
    connection: &Connection,
) -> Result<Option<MainAgent>, HostCatalogError> {
    let row = connection
        .query_row(
            "SELECT agent_id, revision FROM host_main_agent WHERE singleton = 1",
            [],
            |row| Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?)),
        )
        .optional()?;
    row.map(|(agent, revision)| {
        let main_agent_id = Uuid::parse_str(&agent).map_err(|error| {
            HostCatalogError::Invalid(format!("invalid main agent id `{agent}`: {error}"))
        })?;
        Ok(MainAgent {
            main_agent_id,
            revision,
        })
    })
    .transpose()
}

/// The Host's main agent, `None` only while the Host has no agent. Plugins
/// read it for what only the main agent is shown and whose settings speak for
/// the Host; it grants no authority.
pub(crate) fn current(connection: &Connection) -> Result<Option<AgentId>, HostCatalogError> {
    Ok(read(connection)?.map(|main| AgentId::from_uuid(main.main_agent_id)))
}

/// One logical change. Retry with this same identity after an uncertain
/// response; a new identity represents a new change.
#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MainAgentChange {
    pub operation_id: Uuid,
    pub expected_revision: i64,
    pub agent_id: Uuid,
}

#[derive(Debug, Error)]
#[non_exhaustive]
pub enum MainAgentError {
    #[error("this caller does not own the configured Host")]
    Forbidden,
    #[error("the main agent changed; read its current revision before changing it")]
    Stale,
    #[error("this operation ID was already used for a different main agent change")]
    OperationReused,
    #[error("agent {0} is not on this Host")]
    AgentNotFound(Uuid),
    #[error(transparent)]
    Catalog(#[from] HostCatalogError),
    #[error("main agent storage task failed: {0}")]
    Background(#[from] tokio::task::JoinError),
}

impl From<rusqlite::Error> for MainAgentError {
    fn from(error: rusqlite::Error) -> Self {
        Self::Catalog(error.into())
    }
}

impl From<serde_json::Error> for MainAgentError {
    fn from(error: serde_json::Error) -> Self {
        Self::Catalog(HostCatalogError::Invalid(format!(
            "invalid main agent receipt: {error}"
        )))
    }
}

/// The owner's control of the main agent, without constructing an execution
/// Host. The trusted adapter authenticates the principal; a request body
/// cannot supply caller identity.
#[derive(Clone)]
pub struct HostMainAgentControl {
    database: PathBuf,
    host_id: Uuid,
    owner: Uuid,
}

impl HostMainAgentControl {
    /// Pins existing storage and its configured human owner. Does not migrate,
    /// initialize a Host, discover models, or acquire execution ownership.
    /// # Errors
    /// Returns missing/incompatible storage or a different Host identity.
    pub fn open(root: &Path, host_id: Uuid, owner: Uuid) -> Result<Self, LocalHostError> {
        let root = std::fs::canonicalize(root)?;
        if HostObserver::open(&root)?.host_id() != host_id {
            return Err(HostCatalogError::Invalid(
                "configured Host identity does not match storage".to_owned(),
            )
            .into());
        }
        Ok(Self {
            database: root.join(catalog::HOST_DATABASE),
            host_id,
            owner,
        })
    }

    /// Names another main agent from the revision the caller read. The change
    /// and its receipt commit in one transaction; an identical retry, even
    /// after a later change, returns the original receipt. Naming the agent
    /// that is already main returns the current revision unchanged. Cancelling this
    /// future does not prove rollback: retry the identical request.
    /// # Errors
    /// Rejects other principals, a replaced Host, a stale revision, an agent
    /// not on the Host, an operation ID reused with another body, and storage
    /// failures.
    pub async fn set(
        &self,
        authenticated_principal: Uuid,
        change: MainAgentChange,
    ) -> Result<MainAgent, MainAgentError> {
        if authenticated_principal != self.owner {
            return Err(MainAgentError::Forbidden);
        }
        let database = self.database.clone();
        let (host_id, owner) = (self.host_id, self.owner);
        tokio::task::spawn_blocking(move || apply(&database, host_id, owner, change)).await?
    }
}

fn apply(
    database: &Path,
    host_id: Uuid,
    principal: Uuid,
    change: MainAgentChange,
) -> Result<MainAgent, MainAgentError> {
    let mut connection = catalog::open_verified(database)?;
    let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let stored: String = transaction.query_row(
        "SELECT host_id FROM host_identity WHERE singleton = 1",
        [],
        |row| row.get(0),
    )?;
    if stored != host_id.to_string() {
        return Err(HostCatalogError::Invalid(
            "Host identity changed; reconnect explicitly".to_owned(),
        )
        .into());
    }
    let request = serde_json::to_string(&change)?;
    let receipt: Option<(String, String, String)> = transaction
        .query_row(
            "SELECT principal_id, request_json, result_json
             FROM host_main_agent_operations WHERE operation_id = ?1",
            [change.operation_id.to_string()],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .optional()?;
    if let Some((owner, original, result)) = receipt {
        if owner != principal.to_string() || original != request {
            return Err(MainAgentError::OperationReused);
        }
        return Ok(serde_json::from_str(&result)?);
    }
    if !store::exists(&transaction, AgentId::from_uuid(change.agent_id))? {
        return Err(MainAgentError::AgentNotFound(change.agent_id));
    }
    let current = read(&transaction)?.ok_or_else(|| {
        HostCatalogError::Invalid("the Host has agents but no main agent".to_owned())
    })?;
    if current.revision != change.expected_revision {
        return Err(MainAgentError::Stale);
    }
    // Naming the agent that is already main changes nothing, so the revision
    // stays; the receipt is still written so a retry replays this answer.
    let next = if current.main_agent_id == change.agent_id {
        current
    } else {
        let next = MainAgent {
            main_agent_id: change.agent_id,
            revision: current
                .revision
                .checked_add(1)
                .ok_or(MainAgentError::Stale)?,
        };
        transaction.execute(
            "UPDATE host_main_agent SET agent_id = ?1, revision = ?2 WHERE singleton = 1",
            params![next.main_agent_id.to_string(), next.revision],
        )?;
        next
    };
    transaction.execute(
        "INSERT INTO host_main_agent_operations(operation_id, principal_id, request_json, result_json)
         VALUES (?1, ?2, ?3, ?4)",
        params![
            change.operation_id.to_string(),
            principal.to_string(),
            request,
            serde_json::to_string(&next)?
        ],
    )?;
    transaction.commit()?;
    Ok(next)
}
