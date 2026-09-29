//! Personal Host inventory, independent of model, credential and surface startup.

use std::path::{Path, PathBuf};

use serde::Serialize;
use uuid::Uuid;

use super::catalog::{self, HostCatalogError};
use crate::LocalHostError;

mod inventory;
mod sessions;
#[cfg(test)]
mod tests;

pub use inventory::{
    ObservedAgent, ObservedAutomation, ObservedConnection, ObservedPlugin, ObservedRegistryFailure,
    ObservedSharedRegistry, ObservedSkill,
};
pub use sessions::{
    ObservedOperation, ObservedOperationState, ObservedSession, ObservedSessionState,
};

/// A metadata snapshot, never a copy of credentials, transcripts or model context.
/// Catalog collections share one read transaction. Sessions are observed afterward
/// in individual read transactions; this is not a globally atomic execution view.
#[derive(Debug, Serialize)]
pub struct HostObservation {
    pub host_id: Uuid,
    pub agents: Vec<ObservedAgent>,
    pub sessions: Vec<ObservedSession>,
    pub automations: Vec<ObservedAutomation>,
    pub connections: Vec<ObservedConnection>,
    pub plugins: Vec<ObservedPlugin>,
    pub skills: Vec<ObservedSkill>,
    pub shared_registry: Option<ObservedSharedRegistry>,
}

/// Read access to one existing Host, pinned to its durable identity. It cannot
/// initialize a Host, acquire execution ownership or construct a runtime.
#[derive(Clone)]
pub struct HostObserver {
    root: PathBuf,
    host_id: Uuid,
}

impl HostObserver {
    /// Opens an existing, compatible Host without migrations or model discovery.
    ///
    /// # Errors
    /// Returns missing/incompatible catalog, invalid identity or filesystem errors.
    pub fn open(root: &Path) -> Result<Self, LocalHostError> {
        let root = std::fs::canonicalize(root)?;
        let db = catalog::open_read_only(&root.join(catalog::HOST_DATABASE))?;
        let host_id = identity(&db)?;
        Ok(Self { root, host_id })
    }

    #[must_use]
    pub const fn host_id(&self) -> Uuid {
        self.host_id
    }

    /// Projects committed metadata. Session failures are explicit per-session
    /// states; catalog failure never becomes a successful empty inventory.
    /// Cancelling this future can leave its read-only blocking operation finishing.
    ///
    /// # Errors
    /// Returns catalog corruption, Host identity replacement or background errors.
    pub async fn snapshot(&self) -> Result<HostObservation, LocalHostError> {
        let observer = self.clone();
        tokio::task::spawn_blocking(move || observer.read()).await?
    }

    fn read(&self) -> Result<HostObservation, LocalHostError> {
        let mut db = catalog::open_read_only(&self.root.join(catalog::HOST_DATABASE))?;
        let tx = db.transaction().map_err(HostCatalogError::from)?;
        if identity(&tx)? != self.host_id {
            return Err(HostCatalogError::Invalid(
                "Host identity changed; reconnect explicitly".to_owned(),
            )
            .into());
        }
        let mut result = HostObservation {
            host_id: self.host_id,
            agents: inventory::agents(&tx)?,
            sessions: Vec::new(),
            automations: inventory::automations(&tx)?,
            connections: inventory::connections(&tx)?,
            plugins: inventory::plugins(&tx)?,
            skills: inventory::skills(&tx)?,
            shared_registry: inventory::shared_registry(&tx)?,
        };
        tx.commit().map_err(HostCatalogError::from)?;
        result.sessions = sessions::read(&self.root.join("sessions"), &result.agents)?;
        result.agents.sort_by_key(|agent| agent.id);
        Ok(result)
    }
}

fn identity(db: &rusqlite::Connection) -> Result<Uuid, HostCatalogError> {
    let value: String = db.query_row(
        "SELECT host_id FROM host_identity WHERE singleton=1",
        [],
        |r| r.get(0),
    )?;
    parse_id(&value)
}

fn parse_id(value: &str) -> Result<Uuid, HostCatalogError> {
    Uuid::parse_str(value)
        .map_err(|e| HostCatalogError::Invalid(format!("invalid stored identity: {e}")))
}
