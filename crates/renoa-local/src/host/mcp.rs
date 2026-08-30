use crate::{
    AgentProfileId,
    mcp::{McpAuthorizationResolver, McpCatalogSnapshot, discover},
};
use tokio_util::sync::CancellationToken;

use super::{LocalHost, LocalHostError};

#[cfg(test)]
mod tests;

impl LocalHost {
    /// Durably registers one direct no-auth MCP integration and connection.
    ///
    /// Repeating the same identities and endpoint is a no-op. Reusing either
    /// identity for different configuration fails without changing storage.
    ///
    /// # Errors
    ///
    /// Returns validation, conflict, storage, or background-task failures.
    pub async fn register_direct_mcp_connection(
        &self,
        integration_id: &str,
        connection_id: &str,
        endpoint: &str,
    ) -> Result<(), LocalHostError> {
        let store = self.config.mcp_catalog.clone();
        let integration_id = integration_id.to_owned();
        let connection_id = connection_id.to_owned();
        let endpoint = endpoint.to_owned();
        tokio::task::spawn_blocking(move || {
            store.register_direct_connection(&integration_id, &connection_id, &endpoint)
        })
        .await??;
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
        integration_id: &str,
        connection_id: &str,
        endpoint: &str,
        hostname: &str,
        account: &str,
    ) -> Result<(), LocalHostError> {
        let store = self.config.mcp_catalog.clone();
        let integration_id = integration_id.to_owned();
        let connection_id = connection_id.to_owned();
        let endpoint = endpoint.to_owned();
        let hostname = hostname.to_owned();
        let account = account.to_owned();
        tokio::task::spawn_blocking(move || {
            store.register_gh_cli_connection(
                &integration_id,
                &connection_id,
                &endpoint,
                &hostname,
                &account,
            )
        })
        .await??;
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
        let authorizations = McpAuthorizationResolver::new(
            &self.config.mcp_catalog,
            self.config.mcp_adapter.clone(),
            self.config.mcp_credentials.clone(),
        );
        let authorization = authorizations
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

    /// Enables one discovered MCP connection for an exact registered profile.
    ///
    /// # Errors
    ///
    /// Returns when the connection or tool is missing or storage cannot commit.
    pub async fn enable_profile_mcp_connection(
        &self,
        profile_id: &AgentProfileId,
        connection_id: &str,
    ) -> Result<(), LocalHostError> {
        self.profile(profile_id)?;
        let store = self.config.mcp_catalog.clone();
        let profile_id = profile_id.clone();
        let connection_id = connection_id.to_owned();
        tokio::task::spawn_blocking(move || {
            store.enable_profile_connection(profile_id.as_str(), &connection_id)
        })
        .await??;
        Ok(())
    }

    /// Lists the MCP connections currently enabled for an exact registered profile.
    ///
    /// # Errors
    ///
    /// Returns invalid storage or background-task failures.
    pub async fn profile_mcp_connection_ids(
        &self,
        profile_id: &AgentProfileId,
    ) -> Result<Vec<String>, LocalHostError> {
        self.profile(profile_id)?;
        let store = self.config.mcp_catalog.clone();
        let profile_id = profile_id.clone();
        Ok(
            tokio::task::spawn_blocking(move || store.profile_connection_ids(profile_id.as_str()))
                .await??,
        )
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
