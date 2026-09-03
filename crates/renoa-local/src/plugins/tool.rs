use std::{path::PathBuf, sync::Arc};

use renoa_agent::{BoxFuture, Tool, ToolCall, ToolError, ToolOutput, ToolSpec, ToolUpdates};
use renoa_agent_loop::AgentToolBinding;
use renoa_kernel::{CommandId, EffectRecovery, SessionId};
use serde::Serialize;
use tokio_util::sync::CancellationToken;

mod actions;
mod contract;
mod inventory;
mod output;
#[cfg(test)]
mod tests;

use super::{ExtensionAddRequest, ExtensionConnectionRequest, PluginCredential, PluginManager};
use crate::{AgentProfileId, mcp::oauth_operation_id};
use actions::{ConnectRequest, ExtensionInvocation};
use contract::{AddSourceInput, CredentialInput, ManageInput, manage_tool_spec, resolve_source};
use inventory::{ExtensionListPage, MAX_LIST_LIMIT};
use output::{json_output, plugin_error, registry_error_output};

const TOOL_NAME: &str = "extension_manage";
const BINDING_REVISION: &str = "renoa-extension-manager-v14";

pub(crate) fn profile_plugin_binding(
    profile_id: AgentProfileId,
    manager: PluginManager,
    workspace: PathBuf,
    session_id: SessionId,
    command_id: Option<CommandId>,
) -> AgentToolBinding {
    AgentToolBinding::new(
        BINDING_REVISION,
        Arc::new(ManageTool::for_session(
            profile_id, manager, workspace, session_id, command_id,
        )),
        EffectRecovery::SafeToReplay,
    )
}

struct ManageTool {
    profile_id: AgentProfileId,
    manager: PluginManager,
    workspace: PathBuf,
    session_id: SessionId,
    command_id: Option<CommandId>,
    spec: ToolSpec,
}

impl ManageTool {
    fn for_session(
        profile_id: AgentProfileId,
        manager: PluginManager,
        workspace: PathBuf,
        session_id: SessionId,
        command_id: Option<CommandId>,
    ) -> Self {
        Self {
            profile_id,
            manager,
            workspace,
            session_id,
            command_id,
            spec: manage_tool_spec(TOOL_NAME),
        }
    }

    #[cfg(test)]
    fn new(manager: PluginManager, workspace: PathBuf) -> Self {
        Self::for_session(
            AgentProfileId::new(crate::ALPHA_PROFILE_ID).expect("valid Alpha profile id"),
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
            let input: ManageInput = serde_json::from_value(call.arguments).map_err(|error| {
                ToolError::invalid_input(format!(
                    "{TOOL_NAME} arguments are invalid for the selected action: {error}"
                ))
            })?;
            require_active(&cancellation)?;
            self.execute_input(input, &operation_id, cancellation, updates)
                .await
        })
    }
}

impl ManageTool {
    async fn execute_input(
        &self,
        input: ManageInput,
        operation_id: &str,
        cancellation: CancellationToken,
        updates: ToolUpdates,
    ) -> Result<ToolOutput, ToolError> {
        match input {
            ManageInput::Search { query } => self.search_registry(&query, cancellation).await,
            ManageInput::Lookup {
                registry_name,
                registry_version,
            } => {
                self.lookup_registry(&registry_name, &registry_version, cancellation)
                    .await
            }
            ManageInput::Add {
                source,
                server,
                connection,
                credential,
                replace,
            } => {
                self.add_input(
                    source,
                    server,
                    connection,
                    credential,
                    replace,
                    ExtensionInvocation::new(operation_id, cancellation, &updates),
                )
                .await
            }
            ManageInput::Inspect { source_path } => {
                let source = resolve_source(&self.workspace, source_path)?;
                let inspection = self
                    .manager
                    .inspect(source)
                    .await
                    .map_err(|error| plugin_error(error, false))?;
                json_output(&inspection)
            }
            ManageInput::Install {
                source_path,
                expected_digest,
            } => {
                let source = resolve_source(&self.workspace, source_path)?;
                let installed = self
                    .manager
                    .install(source, expected_digest)
                    .await
                    .map_err(|error| plugin_error(error, true))?;
                json_output(&installed)
            }
            ManageInput::List { cursor, limit } => self.list(cursor.as_deref(), limit).await,
            ManageInput::Connect {
                package_digest,
                server,
                connection,
                credential,
                replace,
                required_scope,
            } => {
                actions::connect(
                    self,
                    ConnectRequest {
                        package_digest,
                        server,
                        connection,
                        credential: credential.map_or(PluginCredential::None, Into::into),
                        replace,
                        required_scope,
                    },
                    ExtensionInvocation::new(operation_id, cancellation, &updates),
                )
                .await
            }
            ManageInput::Authorize {
                connection,
                restart,
                required_scope,
            } => {
                actions::authorize(
                    self,
                    connection,
                    restart,
                    required_scope,
                    ExtensionInvocation::new(operation_id, cancellation, &updates),
                )
                .await
            }
            ManageInput::Disconnect { connection } => self.disconnect(connection).await,
            ManageInput::Enable { connection } => self.enable(connection).await,
        }
    }

    async fn add_input(
        &self,
        source: AddSourceInput,
        server: Option<String>,
        connection: Option<String>,
        credential: Option<CredentialInput>,
        replace: bool,
        invocation: ExtensionInvocation<'_>,
    ) -> Result<ToolOutput, ToolError> {
        let source = source.into_source(&self.workspace)?;
        let connection =
            if server.is_some() || connection.is_some() || credential.is_some() || replace {
                Some(ExtensionConnectionRequest::new(
                    connection,
                    server,
                    credential.map_or(PluginCredential::None, Into::into),
                    replace,
                ))
            } else {
                None
            };
        actions::add(
            self,
            ExtensionAddRequest::new(source, connection),
            invocation,
        )
        .await
    }

    async fn search_registry(
        &self,
        query: &str,
        cancellation: CancellationToken,
    ) -> Result<ToolOutput, ToolError> {
        match self.manager.search_registry(query, cancellation).await {
            Ok(result) => json_output(&RegistryActionOutput {
                action: "search",
                installed: false,
                result: &result,
            }),
            Err(error) => registry_error_output(error),
        }
    }

    async fn list(&self, cursor: Option<&str>, limit: usize) -> Result<ToolOutput, ToolError> {
        if !(1..=MAX_LIST_LIMIT).contains(&limit) {
            return Err(ToolError::invalid_input(format!(
                "list limit must be between 1 and {MAX_LIST_LIMIT}"
            )));
        }
        let packages = self
            .manager
            .list_report()
            .await
            .map_err(|error| plugin_error(error, false))?;
        let connections = self
            .manager
            .connection_statuses(&self.profile_id)
            .await
            .map_err(|error| plugin_error(error, false))?;
        let skill_sources = self
            .manager
            .skill_source_reports(&self.profile_id)
            .await
            .map_err(|error| plugin_error(error, false))?;
        let page = ExtensionListPage::new(&packages, &connections, &skill_sources, cursor, limit)?;
        json_output(&page)
    }

    async fn disconnect(&self, connection: String) -> Result<ToolOutput, ToolError> {
        let catalog_retained = self
            .manager
            .disconnect_profile(&self.profile_id, connection.clone())
            .await
            .map_err(|error| plugin_error(error, true))?;
        json_output(&DisconnectedOutput {
            status: "disconnected",
            connection,
            catalog_retained,
            enabled_for_profile: false,
        })
    }

    async fn enable(&self, connection: String) -> Result<ToolOutput, ToolError> {
        self.manager
            .enable_profile(&self.profile_id, connection.clone())
            .await
            .map_err(|error| plugin_error(error, true))?;
        json_output(&EnabledOutput {
            status: "enabled",
            connection,
            catalog_retained: true,
            enabled_for_profile: true,
        })
    }

    async fn lookup_registry(
        &self,
        registry_name: &str,
        registry_version: &str,
        cancellation: CancellationToken,
    ) -> Result<ToolOutput, ToolError> {
        match self
            .manager
            .lookup_registry(registry_name, registry_version, cancellation)
            .await
        {
            Ok(result) => json_output(&RegistryActionOutput {
                action: "lookup",
                installed: false,
                result: &result,
            }),
            Err(error) => registry_error_output(error),
        }
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
    enabled_for_profile: bool,
}

#[derive(Serialize)]
struct EnabledOutput {
    status: &'static str,
    connection: String,
    catalog_retained: bool,
    enabled_for_profile: bool,
}

#[derive(Serialize)]
struct RegistryActionOutput<'a, T> {
    action: &'static str,
    installed: bool,
    #[serde(flatten)]
    result: &'a T,
}

fn require_active(cancellation: &CancellationToken) -> Result<(), ToolError> {
    if cancellation.is_cancelled() {
        Err(ToolError::cancelled(
            "extension management was cancelled",
            false,
        ))
    } else {
        Ok(())
    }
}
