use super::PluginManager;
use crate::{
    ALPHA_PROFILE_ID, mcp::McpConnectionStatus, plugins::PluginError, skills::SkillSourceReport,
};

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

    pub(crate) async fn skill_source_reports(&self) -> Result<Vec<SkillSourceReport>, PluginError> {
        let skills = self.skills.clone();
        Ok(
            tokio::task::spawn_blocking(move || skills.plugin_source_reports(ALPHA_PROFILE_ID))
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

    pub(crate) async fn enable_alpha(
        &self,
        connection_id: impl Into<String>,
    ) -> Result<(), PluginError> {
        let catalog = self.mcp_catalog.clone();
        let connection_id = connection_id.into();
        Ok(tokio::task::spawn_blocking(move || {
            catalog.enable_alpha_connection(ALPHA_PROFILE_ID, &connection_id)
        })
        .await??)
    }
}
