use std::path::PathBuf;

use renoa_kernel::AgentId;
use uuid::Uuid;

use super::{LocalHost, LocalHostError, catalog, definition::MAX_AGENT_PAGE};
use crate::AgentDefinition;

impl LocalHost {
    /// Returns the durable identity of this Host data root.
    ///
    /// # Errors
    /// Returns catalog storage or identity corruption errors.
    pub async fn host_id(&self) -> Result<Uuid, LocalHostError> {
        let database = self.config.database.clone();
        tokio::task::spawn_blocking(move || {
            let connection = catalog::open_verified(&database)?;
            let value: String = connection
                .query_row(
                    "SELECT host_id FROM host_identity WHERE singleton = 1",
                    [],
                    |row| row.get(0),
                )
                .map_err(catalog::HostCatalogError::from)?;
            parse_uuid(&value)
        })
        .await?
    }

    /// Lists canonical agent definitions in identity order.
    ///
    /// No model or diagnostic store is required. Deleted sessions do not delete
    /// their owning agents.
    ///
    /// # Errors
    /// Returns definition identity, corruption, or catalog storage errors.
    pub async fn list_agents(&self) -> Result<Vec<AgentDefinition>, LocalHostError> {
        let mut agents = Vec::new();
        let mut cursor = None;
        loop {
            let page = self.list_agent_definitions(cursor, MAX_AGENT_PAGE).await?;
            cursor = page.next_cursor;
            agents.extend(page.agents);
            if cursor.is_none() {
                break;
            }
        }
        Ok(agents)
    }

    /// Opens one agent's Host-owned workspace, separate from a surface's own
    /// workspace. Scheduled runs and headless surfaces use it so one agent's
    /// files never mix with another's.
    ///
    /// # Errors
    /// Returns an unknown agent or filesystem error.
    pub async fn agent_workspace(&self, id: AgentId) -> Result<PathBuf, LocalHostError> {
        self.require_agent(id).await?;
        let root = self
            .config
            .sessions
            .parent()
            .ok_or_else(|| {
                LocalHostError::InvalidRequest("Host sessions have no data root".to_owned())
            })?
            .join("agent-workspaces");
        tokio::task::spawn_blocking(move || {
            let path = root.join(id.to_string());
            std::fs::create_dir_all(&path)?;
            Ok(std::fs::canonicalize(path)?)
        })
        .await?
    }
}

fn parse_uuid(value: &str) -> Result<Uuid, LocalHostError> {
    Uuid::parse_str(value).map_err(|error| {
        catalog::HostCatalogError::Invalid(format!("invalid stored identity: {error}")).into()
    })
}
