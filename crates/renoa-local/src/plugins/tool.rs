use std::{path::PathBuf, sync::Arc};

use renoa_agent::{BoxFuture, Tool, ToolCall, ToolError, ToolOutput, ToolSpec, ToolUpdates};
use renoa_agent_loop::AgentToolBinding;
use renoa_kernel::EffectRecovery;
use serde::Serialize;
use tokio_util::sync::CancellationToken;

mod contract;
mod output;
#[cfg(test)]
mod tests;

use super::catalog::CatalogError;
use super::{
    CatalogCandidate, ExtensionAddRequest, ExtensionConnectionRequest, PluginCredential,
    PluginError, PluginManager,
    manager::{ExtensionConnectionOutcome, ExtensionSourceReceipt},
};
use crate::mcp::{McpAdapterError, McpHostError};
use contract::{ManageInput, manage_tool_spec, resolve_source};
use output::{
    InstalledConnectionFailure, catalog_failure_output, installed_connection_failure_output,
    json_output, plugin_error, remote_mcp_error_output,
};

const TOOL_NAME: &str = "extension_manage";
const BINDING_REVISION: &str = "renoa-extension-manager-v2";

pub(crate) fn alpha_plugin_binding(manager: PluginManager, workspace: PathBuf) -> AgentToolBinding {
    AgentToolBinding::new(
        BINDING_REVISION,
        Arc::new(ManageTool::new(manager, workspace)),
        EffectRecovery::SafeToReplay,
    )
}

struct ManageTool {
    manager: PluginManager,
    workspace: PathBuf,
    spec: ToolSpec,
}

impl ManageTool {
    fn new(manager: PluginManager, workspace: PathBuf) -> Self {
        Self {
            manager,
            workspace,
            spec: manage_tool_spec(TOOL_NAME),
        }
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
        _updates: ToolUpdates,
    ) -> BoxFuture<'_, Result<ToolOutput, ToolError>> {
        Box::pin(async move {
            if call.name != TOOL_NAME {
                return Err(ToolError::invalid_input(format!(
                    "tool binding `{TOOL_NAME}` cannot execute call for `{}`",
                    call.name
                )));
            }
            let input: ManageInput = serde_json::from_value(call.arguments).map_err(|error| {
                ToolError::invalid_input(format!(
                    "{TOOL_NAME} arguments are invalid for the selected action: {error}"
                ))
            })?;
            require_active(&cancellation)?;
            self.execute_input(input, cancellation).await
        })
    }
}

impl ManageTool {
    async fn execute_input(
        &self,
        input: ManageInput,
        cancellation: CancellationToken,
    ) -> Result<ToolOutput, ToolError> {
        match input {
            ManageInput::Search { query } => self.search(&query, cancellation).await,
            ManageInput::Add {
                source,
                server,
                connection,
                credential,
            } => {
                let source = source.into_source(&self.workspace)?;
                let connection = if server.is_some() || connection.is_some() || credential.is_some()
                {
                    Some(ExtensionConnectionRequest::new(
                        connection,
                        server,
                        credential.map_or(PluginCredential::None, Into::into),
                    ))
                } else {
                    None
                };
                self.add(ExtensionAddRequest::new(source, connection), cancellation)
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
            ManageInput::List => {
                let installed = self
                    .manager
                    .list()
                    .await
                    .map_err(|error| plugin_error(error, false))?;
                json_output(&installed)
            }
            ManageInput::Connect {
                package_digest,
                server,
                connection,
                credential,
            } => {
                let snapshot = match self
                    .manager
                    .connect_alpha(
                        &package_digest,
                        &server,
                        &connection,
                        credential.map_or(PluginCredential::None, Into::into),
                        cancellation,
                    )
                    .await
                {
                    Ok(snapshot) => snapshot,
                    Err(PluginError::Mcp(McpHostError::Adapter(McpAdapterError::Remote(
                        remote,
                    )))) => return remote_mcp_error_output(&remote),
                    Err(error) => return Err(plugin_error(error, true)),
                };
                json_output(&ConnectionOutput {
                    package_digest,
                    server,
                    connection,
                    catalog_digest: snapshot.digest().to_owned(),
                    tools: snapshot.tools().len(),
                    rejected_tools: snapshot.rejected_tools().len(),
                })
            }
        }
    }

    async fn search(
        &self,
        query: &str,
        cancellation: CancellationToken,
    ) -> Result<ToolOutput, ToolError> {
        let candidates = match self.manager.search_catalog(query, cancellation).await {
            Ok(candidates) => candidates,
            Err(PluginError::Catalog(CatalogError::Remote(failure))) => {
                return catalog_failure_output(&failure);
            }
            Err(error) => return Err(plugin_error(error, false)),
        };
        let next_action = if candidates.is_empty() {
            "No MCP candidate was found. Use web search to find the provider's official MCP documentation, then create or use a local Agent Plugin package; do not guess an endpoint."
        } else {
            "Review the endpoint and auth status, choose the best candidate, then call add with its exact reference."
        };
        json_output(&CatalogSearchOutput {
            source: "integrations.sh",
            candidates,
            next_action,
        })
    }

    async fn add(
        &self,
        request: ExtensionAddRequest,
        cancellation: CancellationToken,
    ) -> Result<ToolOutput, ToolError> {
        let added = match self.manager.add_alpha(request, cancellation).await {
            Ok(added) => added,
            Err(PluginError::Catalog(CatalogError::Remote(failure))) => {
                return catalog_failure_output(&failure);
            }
            Err(error) => return Err(plugin_error(error, true)),
        };
        let (source, candidate, name) = source_output(&added.source);
        match added.connection {
            ExtensionConnectionOutcome::NotRequested => json_output(&InstalledOutput {
                status: "installed",
                source,
                candidate,
                name,
                package_digest: added.installed.digest(),
                metadata: added.installed.metadata(),
                mcp_servers: added.installed.mcp_servers(),
                notices: added.installed.notices(),
                skills: &added.skills,
            }),
            ExtensionConnectionOutcome::Connected {
                id,
                server,
                snapshot,
            } => json_output(&ConnectedOutput {
                status: "connected",
                source,
                candidate,
                name,
                package_digest: added.installed.digest(),
                connection: &id,
                server: &server,
                catalog_digest: snapshot.digest(),
                tools: snapshot.tools().len(),
                rejected_tools: snapshot.rejected_tools().len(),
                notices: added.installed.notices(),
                skills: &added.skills,
            }),
            ExtensionConnectionOutcome::Failed { id, server, error } => {
                installed_connection_failure_output(
                    &InstalledConnectionFailure {
                        source,
                        package_digest: added.installed.digest(),
                        connection: id.as_deref(),
                        server: server.as_deref(),
                        notices: added.installed.notices(),
                        skills: &added.skills,
                    },
                    error,
                )
            }
        }
    }
}

#[derive(Serialize)]
struct CatalogSearchOutput<'a> {
    source: &'static str,
    candidates: Vec<CatalogCandidate>,
    next_action: &'a str,
}

#[derive(Serialize)]
struct ConnectedOutput<'a> {
    status: &'static str,
    source: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    candidate: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    name: Option<&'a str>,
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
    #[serde(skip_serializing_if = "Option::is_none")]
    candidate: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    name: Option<&'a str>,
    package_digest: &'a str,
    metadata: &'a super::PluginMetadata,
    mcp_servers: &'a [super::PluginMcpServer],
    notices: &'a [super::PluginNotice],
    skills: &'a crate::skills::SkillComponentReport,
}

#[derive(Serialize)]
struct ConnectionOutput {
    package_digest: String,
    server: String,
    connection: String,
    catalog_digest: String,
    tools: usize,
    rejected_tools: usize,
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

fn source_output(receipt: &ExtensionSourceReceipt) -> (&'static str, Option<&str>, Option<&str>) {
    match receipt {
        ExtensionSourceReceipt::Catalog { reference, name } => {
            ("integrations.sh", Some(reference), Some(name))
        }
        ExtensionSourceReceipt::Mcp => ("mcp", None, None),
        ExtensionSourceReceipt::Package => ("package", None, None),
    }
}
