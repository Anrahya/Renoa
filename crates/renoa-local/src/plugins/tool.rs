use std::{path::PathBuf, sync::Arc};

use renoa_agent::{BoxFuture, Tool, ToolCall, ToolError, ToolOutput, ToolSpec, ToolUpdates};
use renoa_agent_loop::AgentToolBinding;
use renoa_kernel::{CommandId, EffectRecovery, SessionId};
use serde::Serialize;
use tokio_util::sync::CancellationToken;

mod actions;
mod contract;
pub(super) mod output;
#[cfg(test)]
mod tests;

use super::{
    PluginManager,
    api::{PLUGIN_API_REVISION, PluginInvocation, PluginRequest},
};
use crate::mcp::oauth_operation_id;

use contract::manage_tool_spec;

use output::{plugin_error, remote_mcp_error_output};
use renoa_kernel::AgentId;

const TOOL_NAME: &str = crate::capabilities::PLUGIN_MANAGE;
const BINDING_REVISION: &str = PLUGIN_API_REVISION;

pub(crate) fn agent_plugin_binding(
    agent_id: AgentId,
    manager: PluginManager,
    workspace: PathBuf,
    session_id: SessionId,
    command_id: Option<CommandId>,
) -> AgentToolBinding {
    AgentToolBinding::new(
        BINDING_REVISION,
        Arc::new(ManageTool::for_session(
            agent_id, manager, workspace, session_id, command_id,
        )),
        EffectRecovery::SafeToReplay,
    )
}

struct ManageTool {
    agent_id: AgentId,
    manager: PluginManager,
    workspace: PathBuf,
    session_id: SessionId,
    command_id: Option<CommandId>,
    spec: ToolSpec,
}

impl ManageTool {
    fn for_session(
        agent_id: AgentId,
        manager: PluginManager,
        workspace: PathBuf,
        session_id: SessionId,
        command_id: Option<CommandId>,
    ) -> Self {
        Self {
            agent_id,
            manager,
            workspace,
            session_id,
            command_id,
            spec: manage_tool_spec(TOOL_NAME),
        }
    }

    #[cfg(test)]
    fn new(agent_id: AgentId, manager: PluginManager, workspace: PathBuf) -> Self {
        Self::for_session(
            agent_id,
            manager,
            workspace,
            SessionId::new(),
            Some(CommandId::new()),
        )
    }
}

impl Tool for ManageTool {
    fn spec(&self) -> &ToolSpec {
        &self.spec
    }
    fn execute(
        &self,
        call: ToolCall,
        cancellation: CancellationToken,
        updates: ToolUpdates,
    ) -> BoxFuture<'_, Result<ToolOutput, ToolError>> {
        Box::pin(async move {
            if call.name != TOOL_NAME {
                return Err(ToolError::invalid_input(format!(
                    "tool binding `{TOOL_NAME}` cannot execute call for `{}`",
                    call.name
                )));
            }
            let operation_id = oauth_operation_id(self.session_id, self.command_id, &call.id);
            let input: PluginRequest = serde_json::from_value(call.arguments).map_err(|error| {
                ToolError::invalid_input(format!(
                    "{TOOL_NAME} arguments are invalid for the selected action: {error}"
                ))
            })?;
            self.invoke(input, &operation_id, cancellation, Some(&updates))
                .await
        })
    }
}

impl ManageTool {
    async fn invoke(
        &self,
        request: PluginRequest,
        operation_id: &str,
        cancellation: CancellationToken,
        updates: Option<&ToolUpdates>,
    ) -> Result<ToolOutput, ToolError> {
        let mutating = !matches!(
            request,
            PluginRequest::Inspect { .. } | PluginRequest::List { .. }
        );
        match self
            .manager
            .invoke(
                &self.agent_id,
                &self.workspace,
                request,
                PluginInvocation {
                    operation_id,
                    updates,
                    cancellation,
                },
            )
            .await
        {
            Ok(outcome) => actions::render(outcome),
            Err(super::PluginError::Mcp(crate::mcp::McpHostError::Adapter(
                crate::mcp::McpAdapterError::Remote(remote),
            ))) => remote_mcp_error_output(&remote),
            Err(error) => Err(plugin_error(error, mutating)),
        }
    }

    #[cfg(test)]
    async fn list(&self, cursor: Option<&str>, limit: usize) -> Result<ToolOutput, ToolError> {
        self.invoke(
            PluginRequest::List {
                cursor: cursor.map(str::to_owned),
                limit,
            },
            "fixture-list",
            CancellationToken::new(),
            None,
        )
        .await
    }
}

#[derive(Serialize)]
struct ConnectedOutput<'a> {
    status: &'static str,
    source: &'static str,
    package_digest: &'a str,
    connection: &'a str,
    server: &'a str,
    catalog_digest: &'a str,
    tools: usize,
    rejected_tools: usize,
    notices: &'a [super::PluginNotice],
    skills: &'a crate::skills::SkillComponentReport,
}

#[derive(Serialize)]
struct InstalledOutput<'a> {
    status: &'static str,
    source: &'static str,
    package_digest: &'a str,
    metadata: &'a super::PluginMetadata,
    mcp_servers: &'a [super::PluginMcpServer],
    notices: &'a [super::PluginNotice],
    skills: &'a crate::skills::SkillComponentReport,
}

#[derive(Serialize)]
struct ConnectionOutput {
    status: &'static str,
    package_digest: String,
    server: String,
    connection: String,
    catalog_digest: String,
    tools: usize,
    rejected_tools: usize,
}

#[derive(Serialize)]
struct AuthorizedOutput {
    status: &'static str,
    connection: String,
    catalog_digest: String,
    tools: usize,
    rejected_tools: usize,
}

#[derive(Serialize)]
struct DisconnectedOutput {
    status: &'static str,
    connection: String,
    catalog_retained: bool,
    enabled_for_agent: bool,
}

#[derive(Serialize)]
struct EnabledOutput {
    status: &'static str,
    connection: String,
    catalog_retained: bool,
    enabled_for_agent: bool,
}
