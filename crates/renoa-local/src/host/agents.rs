use std::path::PathBuf;

use renoa_kernel::AgentId;
use uuid::Uuid;

use super::{LocalHost, LocalHostError, catalog, definition::MAX_AGENT_PAGE};
use crate::AgentDefinition;

impl LocalHost {
    /// Lists enabled provider models for owner-controlled agent creation.
    /// # Errors
    /// Returns provider discovery failures without changing agent state.
    pub async fn agent_creation_models(&self) -> Result<Vec<crate::ModelChoice>, LocalHostError> {
        super::models::discover_models_for(&self.config, None).await
    }

    /// Exact machine capabilities accepted by canonical agent creation.
    #[must_use]
    pub fn selectable_native_tools(&self) -> Vec<&'static str> {
        crate::capabilities::selectable_names()
    }

    /// The configured default copied into an owner-created agent's request.
    #[must_use]
    pub fn default_agent_model(&self) -> crate::AgentModelSelection {
        crate::AgentModelSelection {
            provider: self.config.initial_provider,
            model: self.config.initial_model.clone(),
            reasoning: self.config.initial_reasoning,
        }
    }

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
        let home = self.config.home.clone();
        tokio::task::spawn_blocking(move || Ok(home.initialize_agent_workspace(&id.to_string())?))
            .await?
    }
}

fn parse_uuid(value: &str) -> Result<Uuid, LocalHostError> {
    Uuid::parse_str(value).map_err(|error| {
        catalog::HostCatalogError::Invalid(format!("invalid stored identity: {error}")).into()
    })
}
