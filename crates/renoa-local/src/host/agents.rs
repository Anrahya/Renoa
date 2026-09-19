use renoa_kernel::AgentId;
use serde::Serialize;
use uuid::Uuid;

use super::{LocalHost, LocalHostError, catalog, definition::MAX_AGENT_PAGE};
use crate::{AgentDefinition, AgentProfileId};

/// A durable agent, independent of its sessions and the process running them.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct AgentRecord {
    pub id: AgentId,
    pub profile: AgentProfileId,
    pub name: String,
    pub created_by: Option<AgentId>,
}

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

    /// Looks up one canonical agent definition without discovering a model or
    /// constructing a runtime.
    ///
    /// # Errors
    /// Returns catalog storage or definition corruption errors.
    pub async fn agent(&self, id: AgentId) -> Result<Option<AgentDefinition>, LocalHostError> {
        self.agent_definition(id).await
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
        let mut after = None;
        loop {
            let page = self.list_agent_definitions(after, MAX_AGENT_PAGE).await?;
            let Some(last) = page.last().map(|agent| agent.id) else {
                break;
            };
            let complete = page.len() < MAX_AGENT_PAGE;
            agents.extend(page);
            if complete {
                break;
            }
            after = Some(last);
        }
        Ok(agents)
    }
}

fn parse_uuid(value: &str) -> Result<Uuid, LocalHostError> {
    Uuid::parse_str(value).map_err(|error| {
        catalog::HostCatalogError::Invalid(format!("invalid stored identity: {error}")).into()
    })
}
