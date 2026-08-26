use crate::{
    ALPHA_PROFILE_ID,
    mcp::{AlphaMcpTool, McpCatalogSnapshot, discover},
};

use super::{LocalHost, LocalHostError};

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
        let endpoint =
            tokio::task::spawn_blocking(move || store.connection_endpoint(&stored_connection))
                .await??;
        let adapter = self.config.mcp_adapter.clone().ok_or_else(|| {
            LocalHostError::Configuration(
                "RENOA_MCP_ADAPTER must be set before refreshing an MCP catalog".to_owned(),
            )
        })?;
        let snapshot = discover(&adapter, connection_id, &endpoint).await?;
        let store = self.config.mcp_catalog.clone();
        let stored_snapshot = snapshot.clone();
        tokio::task::spawn_blocking(move || store.publish_catalog(&stored_snapshot)).await??;
        Ok(snapshot)
    }

    /// Selects one currently discovered MCP tool for the Alpha profile.
    ///
    /// # Errors
    ///
    /// Returns when the connection or tool is missing or storage cannot commit.
    pub async fn select_alpha_mcp_tool(
        &self,
        connection_id: &str,
        tool_name: &str,
    ) -> Result<(), LocalHostError> {
        let store = self.config.mcp_catalog.clone();
        let connection_id = connection_id.to_owned();
        let tool_name = tool_name.to_owned();
        tokio::task::spawn_blocking(move || {
            store.select_alpha_tool(ALPHA_PROFILE_ID, &connection_id, &tool_name)
        })
        .await??;
        Ok(())
    }

    /// Loads Alpha's selected MCP tools from the latest complete Host catalogs.
    ///
    /// # Errors
    ///
    /// Fails closed if a selected tool is absent or catalog storage is invalid.
    pub async fn alpha_mcp_tools(&self) -> Result<Vec<AlphaMcpTool>, LocalHostError> {
        let store = self.config.mcp_catalog.clone();
        Ok(tokio::task::spawn_blocking(move || store.alpha_tools(ALPHA_PROFILE_ID)).await??)
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
