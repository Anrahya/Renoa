use super::PluginManager;
use renoa_kernel::AgentId;

use crate::{
    mcp::{McpConnectionStatus, McpToolSummary},
    plugins::PluginError,
    skills::SkillSourceReport,
};

impl PluginManager {
    pub(crate) async fn agent_snapshot(
        &self,
        agent_id: &AgentId,
    ) -> Result<AgentPluginSnapshot, PluginError> {
        let store = self.store.clone();
        let agent_id = agent_id.to_string();
        tokio::task::spawn_blocking(move || {
            let mut connection = store.connection()?;
            let tx =
                connection.transaction_with_behavior(rusqlite::TransactionBehavior::Deferred)?;
            let snapshot = AgentPluginSnapshot {
                activations: crate::plugins::activation::read_activations(&tx, &agent_id)?,
                connections: crate::mcp::McpCatalogStore::agent_connection_statuses_on(
                    &tx, &agent_id,
                )?,
                tools: crate::mcp::McpCatalogStore::agent_tool_summaries_on(&tx, &agent_id)?,
                skills: crate::skills::SkillStore::plugin_source_reports_on(&tx, &agent_id)?,
            };
            tx.commit()?;
            Ok(snapshot)
        })
        .await?
    }

    #[cfg(test)]
    pub(crate) async fn connection_statuses(
        &self,
        agent_id: &AgentId,
    ) -> Result<Vec<McpConnectionStatus>, PluginError> {
        Ok(self.agent_snapshot(agent_id).await?.connections)
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

pub(crate) struct AgentPluginSnapshot {
    pub(crate) activations: Vec<crate::plugins::PluginActivation>,
    pub(crate) connections: Vec<McpConnectionStatus>,
    pub(crate) tools: Vec<McpToolSummary>,
    pub(crate) skills: Vec<SkillSourceReport>,
}
