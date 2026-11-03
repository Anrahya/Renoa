use std::path::PathBuf;

use renoa_kernel::AgentId;
use uuid::Uuid;

use super::{LocalHost, LocalHostError, catalog, definition::MAX_AGENT_PAGE};
use crate::{
    AgentDefinition,
    plugins::{
        PluginError,
        host::{
            HostPluginId,
            owner::heartbeat::{self, OwnerSettings},
            settings,
        },
    },
};

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

    /// Returns the installation root this Host owns.
    #[must_use]
    pub fn home(&self) -> &renoa_home::RenoaHome {
        &self.config.home
    }

    /// The Host catalog this Host reads and writes.
    pub(crate) fn database(&self) -> &std::path::Path {
        &self.config.database
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

    /// How many hours apart `agent`'s heartbeat runs, from its `renoa.owner`
    /// setting or the default; 0 is off. `None` for an agent this Host does
    /// not have.
    ///
    /// # Errors
    /// Returns catalog failures and a stored setting that does not decode.
    pub async fn heartbeat_hours(&self, agent: AgentId) -> Result<Option<u8>, LocalHostError> {
        let database = self.config.database.clone();
        tokio::task::spawn_blocking(move || {
            let db = catalog::open_verified(&database)?;
            if !has_agent(&db, agent)? {
                return Ok(None);
            }
            Ok(Some(heartbeat::hours(&db, agent)?))
        })
        .await?
    }

    /// Sets `agent`'s heartbeat `hours` apart, 1 to 24, or off with 0, once
    /// per `operation`, and returns it as it is now; `None` for an agent this
    /// Host does not have. The scheduler moves the heartbeat before it next
    /// admits a run.
    ///
    /// # Errors
    /// Rejects hours above 24 and a reused operation with other hours, and
    /// returns catalog failures; nothing is stored then.
    pub async fn set_heartbeat_hours(
        &self,
        agent: AgentId,
        hours: u8,
        operation: Uuid,
    ) -> Result<Option<u8>, LocalHostError> {
        let database = self.config.database.clone();
        tokio::task::spawn_blocking(move || {
            if !has_agent(&catalog::open_verified(&database)?, agent)? {
                return Ok(None);
            }
            settings::configure(
                &database,
                agent,
                HostPluginId::Owner,
                OwnerSettings::with_heartbeat(hours)?,
                &format!("heartbeat:{operation}"),
            )?;
            Ok(Some(heartbeat::hours(
                &catalog::open_verified(&database)?,
                agent,
            )?))
        })
        .await?
    }
}

fn parse_uuid(value: &str) -> Result<Uuid, LocalHostError> {
    Uuid::parse_str(value).map_err(|error| {
        catalog::HostCatalogError::Invalid(format!("invalid stored identity: {error}")).into()
    })
}

fn has_agent(db: &rusqlite::Connection, agent: AgentId) -> Result<bool, PluginError> {
    Ok(db.query_row(
        "SELECT EXISTS(SELECT 1 FROM host_agents WHERE agent_id = ?1)",
        [agent.to_string()],
        |row| row.get(0),
    )?)
}
