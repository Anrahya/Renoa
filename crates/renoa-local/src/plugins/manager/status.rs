use super::PluginManager;
use renoa_kernel::AgentId;

use crate::{
    mcp::{McpConnectionStatus, McpToolSummary},
    plugins::PluginError,
    skills::SkillSourceReport,
};

impl PluginManager {
    pub(crate) async fn tool_summaries(
        &self,
        agent_id: &AgentId,
    ) -> Result<Vec<McpToolSummary>, PluginError> {
        let catalog = self.mcp_catalog.clone();
        let agent_id = *agent_id;
        Ok(
            tokio::task::spawn_blocking(move || {
                catalog.agent_tool_summaries(&agent_id.to_string())
            })
            .await??,
        )
    }

    pub(crate) async fn connection_statuses(
        &self,
        agent_id: &AgentId,
    ) -> Result<Vec<McpConnectionStatus>, PluginError> {
        let catalog = self.mcp_catalog.clone();
        let agent_id = *agent_id;
        Ok(tokio::task::spawn_blocking(move || {
            catalog.agent_connection_statuses(&agent_id.to_string())
        })
        .await??)
    }

    pub(crate) async fn skill_source_reports(
        &self,
        agent_id: &AgentId,
    ) -> Result<Vec<SkillSourceReport>, PluginError> {
        let skills = self.skills.clone();
        let agent_id = *agent_id;
        Ok(
            tokio::task::spawn_blocking(move || {
                skills.plugin_source_reports(&agent_id.to_string())
            })
            .await??,
        )
    }

    pub(crate) async fn disconnect_agent(
        &self,
        agent_id: &AgentId,
        connection_id: impl Into<String>,
    ) -> Result<bool, PluginError> {
        let catalog = self.mcp_catalog.clone();
        let agent_id = *agent_id;
        let connection_id = connection_id.into();
        Ok(tokio::task::spawn_blocking(move || {
            catalog.disable_agent_connection(&agent_id.to_string(), &connection_id)
        })
        .await??)
    }

    pub(crate) async fn enable_agent(
        &self,
        agent_id: &AgentId,
        connection_id: impl Into<String>,
    ) -> Result<(), PluginError> {
        let catalog = self.mcp_catalog.clone();
        let agent_id = *agent_id;
        let connection_id = connection_id.into();
        Ok(tokio::task::spawn_blocking(move || {
            catalog.enable_agent_connection(&agent_id.to_string(), &connection_id)
        })
        .await??)
    }
}
