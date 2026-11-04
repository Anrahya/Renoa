//! The apps an agent can use, as the owner sees and switches them: every Host
//! connection with a complete catalog, named by the plugin that serves it,
//! and whether the agent has it. Switching one off takes its tools away from
//! the agent and keeps the connection, its sign-in and its catalog, so
//! switching it back on needs no new sign-in; the plugin's skills stay, as
//! after `plugin_manage` disconnect.

use renoa_kernel::AgentId;
use rusqlite::Connection;
use serde::Serialize;

use super::{LocalHost, LocalHostError, catalog, catalog_error, store};
use crate::mcp::validate_identity;

/// One Host connection, as an agent's owner sees it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct AgentApp {
    /// The connection's id, which switching it names.
    pub connection: String,
    /// The name of the installed plugin that serves it, if any.
    pub name: Option<String>,
    /// What that plugin says it does.
    pub about: Option<String>,
    /// Where the agent reaches it.
    pub endpoint: String,
    /// Whether the agent can use it now.
    pub on: bool,
}

impl LocalHost {
    /// Lists every Host connection with a complete catalog and whether `id`
    /// can use it.
    ///
    /// # Errors
    /// Returns `AgentNotFound` for an unknown agent and catalog storage errors.
    pub async fn agent_apps(&self, id: AgentId) -> Result<Vec<AgentApp>, LocalHostError> {
        let database = self.config.database.clone();
        tokio::task::spawn_blocking(move || {
            let connection = catalog::open_verified(&database)?;
            if !store::exists(&connection, id)? {
                return Err(LocalHostError::AgentNotFound(id));
            }
            read(&connection, id)
        })
        .await?
    }

    /// Gives `id` the connection `connection_id` or takes it away, then lists
    /// the agent's apps. Either is idempotent.
    ///
    /// # Errors
    /// Rejects a malformed connection id, unknown agents or connections,
    /// turning on a connection whose catalog is not complete or whose plugin
    /// the agent cannot select, and storage failures.
    pub async fn set_agent_app(
        &self,
        id: AgentId,
        connection_id: &str,
        on: bool,
    ) -> Result<Vec<AgentApp>, LocalHostError> {
        validate_identity("connection", connection_id)?;
        if on {
            self.enable_agent_connection(id, connection_id).await?;
        } else {
            self.require_agent(id).await?;
            let catalog = self.config.mcp_catalog.clone();
            let agent = id.to_string();
            let connection_id = connection_id.to_owned();
            tokio::task::spawn_blocking(move || {
                catalog.disable_agent_connection(&agent, &connection_id)
            })
            .await??;
        }
        self.agent_apps(id).await
    }
}

fn read(connection: &Connection, id: AgentId) -> Result<Vec<AgentApp>, LocalHostError> {
    // At most one plugin serves an integration: `plugin_mcp_servers` keeps
    // `integration_id` unique.
    let mut statement = connection
        .prepare(
            "SELECT configured.connection_id, plugin.name, plugin.description,
                    integration.endpoint,
                    EXISTS(
                        SELECT 1 FROM host_agent_enabled_mcp_connections AS binding
                        WHERE binding.agent_id = ?1
                          AND binding.connection_id = configured.connection_id
                    )
             FROM mcp_connections AS configured
             JOIN mcp_integrations AS integration
               ON integration.integration_id = configured.integration_id
             JOIN mcp_catalogs AS complete
               ON complete.connection_id = configured.connection_id
             LEFT JOIN plugin_mcp_servers AS server
               ON server.integration_id = configured.integration_id
             LEFT JOIN installed_plugins AS plugin
               ON plugin.plugin_digest = server.plugin_digest
             ORDER BY configured.connection_id",
        )
        .map_err(catalog_error)?;
    statement
        .query_map([id.to_string()], |row| {
            Ok(AgentApp {
                connection: row.get(0)?,
                name: row.get(1)?,
                about: row.get(2)?,
                endpoint: row.get(3)?,
                on: row.get(4)?,
            })
        })
        .map_err(catalog_error)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(catalog_error)
}
