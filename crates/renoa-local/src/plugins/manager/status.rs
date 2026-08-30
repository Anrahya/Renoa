use super::PluginManager;
use crate::{
    AgentProfileId, mcp::McpConnectionStatus, plugins::PluginError, skills::SkillSourceReport,
};

impl PluginManager {
    pub(crate) async fn connection_statuses(
        &self,
        profile_id: &AgentProfileId,
    ) -> Result<Vec<McpConnectionStatus>, PluginError> {
        let catalog = self.mcp_catalog.clone();
        let profile_id = profile_id.clone();
        Ok(tokio::task::spawn_blocking(move || {
            catalog.profile_connection_statuses(profile_id.as_str())
        })
        .await??)
    }

    pub(crate) async fn skill_source_reports(
        &self,
        profile_id: &AgentProfileId,
    ) -> Result<Vec<SkillSourceReport>, PluginError> {
        let skills = self.skills.clone();
        let profile_id = profile_id.clone();
        Ok(
            tokio::task::spawn_blocking(move || skills.plugin_source_reports(profile_id.as_str()))
                .await??,
        )
    }

    pub(crate) async fn disconnect_profile(
        &self,
        profile_id: &AgentProfileId,
        connection_id: impl Into<String>,
    ) -> Result<bool, PluginError> {
        let catalog = self.mcp_catalog.clone();
        let profile_id = profile_id.clone();
        let connection_id = connection_id.into();
        Ok(tokio::task::spawn_blocking(move || {
            catalog.disable_profile_connection(profile_id.as_str(), &connection_id)
        })
        .await??)
    }

    pub(crate) async fn enable_profile(
        &self,
        profile_id: &AgentProfileId,
        connection_id: impl Into<String>,
    ) -> Result<(), PluginError> {
        let catalog = self.mcp_catalog.clone();
        let profile_id = profile_id.clone();
        let connection_id = connection_id.into();
        Ok(tokio::task::spawn_blocking(move || {
            catalog.enable_profile_connection(profile_id.as_str(), &connection_id)
        })
        .await??)
    }
}
