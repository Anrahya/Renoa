//! How each agent looks to its owner: a look the Host picks from parts when
//! the agent is made, never one another agent wears, and a picture the owner
//! may set over it. The Host stores choices by name and pictures as bytes;
//! every app draws them in its own colours.

mod look;
mod picture;

use renoa_kernel::AgentId;
use rusqlite::{Transaction, TransactionBehavior};
use serde::Serialize;

pub(in crate::host) use look::assign;
pub use look::{Cheeks, Eyes, HairBack, HairColour, HairFront, Look, Skin, Tile, Wear};
pub use picture::{AgentPicture, MAX_PICTURE_BYTES, NewPicture, PictureChange, PictureError};

use super::{LocalHost, LocalHostError, catalog, catalog::HostCatalogError};

/// What an app needs to draw an agent: its look, and the revision of the
/// picture set over it, if any, so the app reads the picture only when that
/// revision changed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct AgentAppearance {
    pub look: Look,
    pub picture_revision: Option<String>,
}

/// Schema 54's tables, for a new catalog.
pub(in crate::host) fn initialize(transaction: &Transaction<'_>) -> Result<(), HostCatalogError> {
    look::initialize(transaction)?;
    picture::initialize(transaction)
}

/// Schema 54 for a catalog made before it: the tables, and a look for every
/// agent already made.
pub(in crate::host) fn upgrade(transaction: &Transaction<'_>) -> Result<(), HostCatalogError> {
    look::give_every_agent_a_look(transaction)?;
    picture::initialize(transaction)
}

/// Turns a schema 54 catalog back into schema 53's tables.
#[cfg(test)]
pub(crate) fn restore_schema_53(connection: &rusqlite::Connection) {
    connection
        .execute_batch(
            "DROP TABLE host_agent_picture_operations;
             DROP TABLE host_agent_pictures;
             DROP TABLE host_agent_looks;",
        )
        .expect("restore schema 53");
}

impl LocalHost {
    /// `agent`'s look and picture revision; `None` for an agent this Host does
    /// not have.
    ///
    /// # Errors
    /// Returns catalog failures, and an agent with no look, which only a
    /// damaged catalog holds.
    pub async fn agent_appearance(
        &self,
        agent: AgentId,
    ) -> Result<Option<AgentAppearance>, LocalHostError> {
        let database = self.config.database.clone();
        tokio::task::spawn_blocking(move || {
            let db = catalog::open_verified(&database)?;
            if !exists(&db, agent)? {
                return Ok(None);
            }
            let look = look::read(&db, agent)?
                .ok_or_else(|| HostCatalogError::Invalid(format!("agent {agent} has no look")))?;
            Ok(Some(AgentAppearance {
                look,
                picture_revision: picture::revision(&db, agent)?,
            }))
        })
        .await?
    }

    /// `agent`'s picture, if one is set.
    ///
    /// # Errors
    /// Returns catalog failures.
    pub async fn agent_picture(
        &self,
        agent: AgentId,
    ) -> Result<Option<AgentPicture>, LocalHostError> {
        let database = self.config.database.clone();
        tokio::task::spawn_blocking(move || {
            Ok(picture::read(&catalog::open_verified(&database)?, agent)?)
        })
        .await?
    }

    /// Sets or takes away `agent`'s picture once per `change.operation_id`,
    /// and returns the revision now set; `Ok(None)` for an agent this Host
    /// does not have. Every check runs before the first write, so a refused
    /// change stores nothing.
    ///
    /// # Errors
    /// Refuses a nil operation id, a picture of another type than its bytes,
    /// an empty or oversized one, a stale `expected_revision`, and a reused
    /// operation id; returns catalog failures.
    pub async fn set_agent_picture(
        &self,
        agent: AgentId,
        change: PictureChange,
    ) -> Result<Option<Option<String>>, PictureError> {
        if change.operation_id.is_nil() {
            return Err(PictureError::Invalid(
                "operation_id is nil; send a new UUID the app chooses for this change".to_owned(),
            ));
        }
        let revision = change.picture.as_ref().map(picture::validate).transpose()?;
        let database = self.config.database.clone();
        tokio::task::spawn_blocking(move || {
            let mut db = catalog::open_verified(&database)?;
            let transaction = db.transaction_with_behavior(TransactionBehavior::Immediate)?;
            if !exists(&transaction, agent)? {
                return Ok(None);
            }
            let set = picture::apply(&transaction, agent, &change, revision)?;
            transaction.commit()?;
            Ok(Some(set))
        })
        .await
        .map_err(LocalHostError::from)?
    }
}

fn exists(db: &rusqlite::Connection, agent: AgentId) -> Result<bool, HostCatalogError> {
    Ok(db.query_row(
        "SELECT EXISTS(SELECT 1 FROM host_agents WHERE agent_id = ?1)",
        [agent.to_string()],
        |row| row.get(0),
    )?)
}

#[cfg(test)]
mod tests;
