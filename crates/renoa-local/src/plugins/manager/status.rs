use super::PluginManager;
use crate::{ALPHA_PROFILE_ID, mcp::McpConnectionStatus, plugins::PluginError};

impl PluginManager {
    pub(crate) async fn connection_statuses(
        &self,
    ) -> Result<Vec<McpConnectionStatus>, PluginError> {
        let catalog = self.mcp_catalog.clone();
        Ok(
            tokio::task::spawn_blocking(move || {
                catalog.alpha_connection_statuses(ALPHA_PROFILE_ID)
            })
            .await??,
        )
    }

    pub(crate) async fn disconnect_alpha(
        &self,
        connection_id: impl Into<String>,
    ) -> Result<bool, PluginError> {
        let catalog = self.mcp_catalog.clone();
        let connection_id = connection_id.into();
        Ok(tokio::task::spawn_blocking(move || {
            catalog.disable_alpha_connection(ALPHA_PROFILE_ID, &connection_id)
        })
        .await??)
    }
}
