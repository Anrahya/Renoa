use crate::mcp::{McpCatalogSnapshot, McpConnectionAuth, discover};
use tokio_util::sync::CancellationToken;

use super::{LocalHost, LocalHostError};

#[cfg(test)]
mod tests;

impl LocalHost {
    /// Installs a portable MCP plugin and registers one no-auth connection.
    ///
    /// Repeating the same name, endpoint, and connection converges on one revision.
    /// Discovery and agent activation remain separate operations.
    ///
    /// # Errors
    ///
    /// Returns validation, conflict, storage, or background-task failures.
    pub async fn register_direct_mcp_connection(
        &self,
        plugin_name: &str,
        connection_id: &str,
        endpoint: &str,
    ) -> Result<(), LocalHostError> {
        self.config
            .plugins
            .register_host_mcp(
                plugin_name,
                connection_id,
                endpoint,
                McpConnectionAuth::None,
            )
            .await?;
        Ok(())
    }

    /// Durably registers one MCP connection whose token is resolved from an exact `gh` account.
    ///
    /// Only the hostname and account reference are stored. The token is resolved
    /// just in time and is never written to Host storage.
    ///
    /// # Errors
    ///
    /// Returns validation, conflict, storage, or background-task failures.
    pub async fn register_gh_cli_mcp_connection(
        &self,
        plugin_name: &str,
        connection_id: &str,
        endpoint: &str,
        hostname: &str,
        account: &str,
    ) -> Result<(), LocalHostError> {
        self.config
            .plugins
            .register_host_mcp(
                plugin_name,
                connection_id,
                endpoint,
                McpConnectionAuth::gh_cli(hostname, account)?,
            )
            .await?;
        Ok(())
    }

    /// Discovers and atomically publishes one connection's complete MCP catalog.
    ///
    /// A failed refresh leaves the previous complete snapshot unchanged.
    ///
    /// # Errors
    ///
    /// Returns missing configuration, adapter, protocol, storage, or task failures.
    pub async fn refresh_mcp_catalog(
        &self,
        connection_id: &str,
    ) -> Result<McpCatalogSnapshot, LocalHostError> {
        let store = self.config.mcp_catalog.clone();
        let stored_connection = connection_id.to_owned();
        let connection =
            tokio::task::spawn_blocking(move || store.connection_config(&stored_connection))
                .await??;
        let adapter = self.config.mcp_adapter.clone().ok_or_else(|| {
            LocalHostError::Configuration(
                "RENOA_MCP_ADAPTER must be set before refreshing an MCP catalog".to_owned(),
            )
        })?;
        let operation_id = format!("host-refresh.{}", uuid::Uuid::new_v4());
        let authorization = self
            .config
            .mcp_authorizations
            .resolve(
                connection_id,
                &connection.endpoint,
                &connection.auth,
                &operation_id,
                CancellationToken::new(),
            )
            .await?;
        let snapshot = discover(
            &adapter,
            connection_id,
            &connection.endpoint,
            &connection.request_headers,
            authorization.as_ref(),
        )
        .await?;
        let store = self.config.mcp_catalog.clone();
        let stored_snapshot = snapshot.clone();
        tokio::task::spawn_blocking(move || store.publish_catalog(&stored_snapshot)).await??;
        Ok(snapshot)
    }

    /// Loads one connection's latest complete MCP catalog.
    ///
    /// # Errors
    ///
    /// Returns when the catalog is missing, corrupt, or cannot be read.
    pub async fn mcp_catalog(
        &self,
        connection_id: &str,
    ) -> Result<McpCatalogSnapshot, LocalHostError> {
        let store = self.config.mcp_catalog.clone();
        let connection_id = connection_id.to_owned();
        Ok(tokio::task::spawn_blocking(move || store.load_catalog(&connection_id)).await??)
    }
}
