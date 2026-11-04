//! The apps an agent can use, as the owner sees and switches them: every Host
//! connection, named by the plugin that serves it, and whether the agent has
//! it. Switching one off keeps the connection and its sign-in, so switching it
//! back on needs no new sign-in.

use renoa_kernel::AgentId;
use rusqlite::{Connection, OptionalExtension};
use serde::Serialize;

use super::{LocalHost, LocalHostError, catalog, catalog_error, store};
use crate::mcp::McpHostError;

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
    /// Lists every Host connection and whether `id` can use it.
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
    /// Rejects unknown agents or connections, turning on a connection whose
    /// catalog is not complete or whose plugin the agent has switched off, and
    /// storage failures.
    pub async fn set_agent_app(
        &self,
        id: AgentId,
        connection_id: &str,
        on: bool,
    ) -> Result<Vec<AgentApp>, LocalHostError> {
        if on {
            self.enable_agent_connection(id, connection_id).await?;
        } else {
            let database = self.config.database.clone();
            let connection_id = connection_id.to_owned();
            tokio::task::spawn_blocking(move || disable(&database, id, &connection_id)).await??;
        }
        self.agent_apps(id).await
    }
}

fn disable(
    database: &std::path::Path,
    id: AgentId,
    connection_id: &str,
) -> Result<(), LocalHostError> {
    let mut catalog_connection = catalog::open_verified(database)?;
    let transaction = catalog_connection
        .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
        .map_err(catalog_error)?;
    if !store::exists(&transaction, id)? {
        return Err(LocalHostError::AgentNotFound(id));
    }
    let registered = transaction
        .query_row(
            "SELECT 1 FROM mcp_connections WHERE connection_id = ?1",
            [connection_id],
            |_| Ok(()),
        )
        .optional()
        .map_err(catalog_error)?
        .is_some();
    if !registered {
        return Err(McpHostError::NotFound(format!(
            "connection '{connection_id}' is not registered"
        ))
        .into());
    }
    let mut selected = store::read_connections(&transaction, id)?;
    if selected.remove(connection_id) {
        store::set_connections(&transaction, id, &selected)?;
        transaction.commit().map_err(catalog_error)?;
    }
    Ok(())
}

fn read(connection: &Connection, id: AgentId) -> Result<Vec<AgentApp>, LocalHostError> {
    // A connection a plugin serves takes the name of the first plugin by name
    // that packages its integration.
    let mut statement = connection
        .prepare(
            "SELECT configured.connection_id,
                    (SELECT plugin.name FROM plugin_mcp_servers AS server
                     JOIN installed_plugins AS plugin
                       ON plugin.plugin_digest = server.plugin_digest
                     WHERE server.integration_id = configured.integration_id
                     ORDER BY plugin.name, plugin.plugin_digest LIMIT 1),
                    (SELECT plugin.description FROM plugin_mcp_servers AS server
                     JOIN installed_plugins AS plugin
                       ON plugin.plugin_digest = server.plugin_digest
                     WHERE server.integration_id = configured.integration_id
                     ORDER BY plugin.name, plugin.plugin_digest LIMIT 1),
                    integration.endpoint,
                    EXISTS(
                        SELECT 1 FROM host_agent_enabled_mcp_connections AS binding
                        WHERE binding.agent_id = ?1
                          AND binding.connection_id = configured.connection_id
                    )
             FROM mcp_connections AS configured
             JOIN mcp_integrations AS integration
               ON integration.integration_id = configured.integration_id
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
