use renoa_agent::{ToolError, ToolOutput, ToolUpdates};
use tokio_util::sync::CancellationToken;

use super::{
    AuthorizedOutput, ConnectedOutput, ConnectionOutput, InstalledOutput, ManageTool,
    output::{
        InstalledConnectionFailure, catalog_failure_output, installed_connection_failure_output,
        json_output, plugin_error, remote_mcp_error_output,
    },
};
use crate::{
    mcp::{McpAdapterError, McpCatalogSnapshot, McpHostError},
    plugins::{
        ExtensionAddRequest, InstalledPlugin, PluginCredential, PluginError,
        manager::{ExtensionAddOutcome, ExtensionConnectionOutcome, ExtensionSourceReceipt},
    },
    skills::SkillComponentReport,
};

pub(super) struct ExtensionInvocation<'a> {
    operation_id: &'a str,
    cancellation: CancellationToken,
    updates: &'a ToolUpdates,
}

impl<'a> ExtensionInvocation<'a> {
    pub(super) const fn new(
        operation_id: &'a str,
        cancellation: CancellationToken,
        updates: &'a ToolUpdates,
    ) -> Self {
        Self {
            operation_id,
            cancellation,
            updates,
        }
    }
}

pub(super) struct ConnectRequest {
    pub(super) package_digest: String,
    pub(super) server: String,
    pub(super) connection: String,
    pub(super) credential: PluginCredential,
}

pub(super) async fn connect(
    tool: &ManageTool,
    request: ConnectRequest,
    invocation: ExtensionInvocation<'_>,
) -> Result<ToolOutput, ToolError> {
    let snapshot = match tool
        .manager
        .connect_alpha_operation(
            &request.package_digest,
            &request.server,
            &request.connection,
            request.credential,
            invocation.operation_id,
            invocation.cancellation.clone(),
        )
        .await
    {
        Ok(snapshot) => snapshot,
        Err(PluginError::Mcp(McpHostError::OAuth(
            crate::mcp::McpOAuthError::AuthorizationRequired(_),
        ))) => match authorize_snapshot(tool, &request.connection, false, invocation).await {
            Ok(snapshot) => snapshot,
            Err(PluginError::Mcp(McpHostError::Adapter(McpAdapterError::Remote(remote)))) => {
                return remote_mcp_error_output(&remote);
            }
            Err(error) => return Err(plugin_error(error, true)),
        },
        Err(PluginError::Mcp(McpHostError::Adapter(McpAdapterError::Remote(remote)))) => {
            return remote_mcp_error_output(&remote);
        }
        Err(error) => return Err(plugin_error(error, true)),
    };
    json_output(&ConnectionOutput {
        package_digest: request.package_digest,
        server: request.server,
        connection: request.connection,
        catalog_digest: snapshot.digest().to_owned(),
        tools: snapshot.tools().len(),
        rejected_tools: snapshot.rejected_tools().len(),
    })
}

pub(super) async fn authorize(
    tool: &ManageTool,
    connection: String,
    restart: bool,
    invocation: ExtensionInvocation<'_>,
) -> Result<ToolOutput, ToolError> {
    let snapshot = match authorize_snapshot(tool, &connection, restart, invocation).await {
        Ok(snapshot) => snapshot,
        Err(PluginError::Mcp(McpHostError::Adapter(McpAdapterError::Remote(remote)))) => {
            return remote_mcp_error_output(&remote);
        }
        Err(error) => return Err(plugin_error(error, true)),
    };
    json_output(&AuthorizedOutput {
        status: "authorized",
        connection,
        catalog_digest: snapshot.digest().to_owned(),
        tools: snapshot.tools().len(),
        rejected_tools: snapshot.rejected_tools().len(),
    })
}

async fn authorize_snapshot(
    tool: &ManageTool,
    connection: &str,
    restart: bool,
    invocation: ExtensionInvocation<'_>,
) -> Result<McpCatalogSnapshot, PluginError> {
    tool.manager
        .authorize_alpha(
            connection,
            invocation.operation_id,
            restart,
            Some(invocation.updates),
            invocation.cancellation,
        )
        .await
}

pub(super) async fn add(
    tool: &ManageTool,
    request: ExtensionAddRequest,
    invocation: ExtensionInvocation<'_>,
) -> Result<ToolOutput, ToolError> {
    let added = match tool
        .manager
        .add_alpha(
            request,
            invocation.operation_id,
            invocation.cancellation.clone(),
        )
        .await
    {
        Ok(added) => added,
        Err(PluginError::Catalog(crate::plugins::CatalogError::Remote(failure))) => {
            return catalog_failure_output(&failure);
        }
        Err(error) => return Err(plugin_error(error, true)),
    };
    render_added(tool, added, invocation).await
}

struct AddedExtensionView<'a> {
    source: &'static str,
    candidate: Option<&'a str>,
    name: Option<&'a str>,
    installed: &'a InstalledPlugin,
    skills: &'a SkillComponentReport,
}

async fn render_added(
    tool: &ManageTool,
    added: ExtensionAddOutcome,
    invocation: ExtensionInvocation<'_>,
) -> Result<ToolOutput, ToolError> {
    let (source, candidate, name) = source_output(&added.source);
    let output = AddedExtensionView {
        source,
        candidate,
        name,
        installed: &added.installed,
        skills: &added.skills,
    };
    match added.connection {
        ExtensionConnectionOutcome::NotRequested => installed_output(&output),
        ExtensionConnectionOutcome::Connected {
            id,
            server,
            snapshot,
        } => connected_output(&output, &id, &server, &snapshot),
        ExtensionConnectionOutcome::Failed { id, server, error } => {
            failed_output(tool, &output, id, server, error, invocation).await
        }
    }
}

fn installed_output(extension: &AddedExtensionView<'_>) -> Result<ToolOutput, ToolError> {
    json_output(&InstalledOutput {
        status: "installed",
        source: extension.source,
        candidate: extension.candidate,
        name: extension.name,
        package_digest: extension.installed.digest(),
        metadata: extension.installed.metadata(),
        mcp_servers: extension.installed.mcp_servers(),
        notices: extension.installed.notices(),
        skills: extension.skills,
    })
}

fn connected_output(
    extension: &AddedExtensionView<'_>,
    connection: &str,
    server: &str,
    snapshot: &McpCatalogSnapshot,
) -> Result<ToolOutput, ToolError> {
    json_output(&ConnectedOutput {
        status: "connected",
        source: extension.source,
        candidate: extension.candidate,
        name: extension.name,
        package_digest: extension.installed.digest(),
        connection,
        server,
        catalog_digest: snapshot.digest(),
        tools: snapshot.tools().len(),
        rejected_tools: snapshot.rejected_tools().len(),
        notices: extension.installed.notices(),
        skills: extension.skills,
    })
}

async fn failed_output(
    tool: &ManageTool,
    extension: &AddedExtensionView<'_>,
    connection: Option<String>,
    server: Option<String>,
    error: PluginError,
    invocation: ExtensionInvocation<'_>,
) -> Result<ToolOutput, ToolError> {
    if matches!(
        &error,
        PluginError::Mcp(McpHostError::OAuth(
            crate::mcp::McpOAuthError::AuthorizationRequired(_)
        ))
    ) && let (Some(connection), Some(server)) = (&connection, &server)
    {
        return match authorize_snapshot(tool, connection, false, invocation).await {
            Ok(snapshot) => connected_output(extension, connection, server, &snapshot),
            Err(error) => installed_failure(extension, Some(connection), Some(server), error),
        };
    }
    installed_failure(extension, connection.as_deref(), server.as_deref(), error)
}

fn installed_failure(
    extension: &AddedExtensionView<'_>,
    connection: Option<&str>,
    server: Option<&str>,
    error: PluginError,
) -> Result<ToolOutput, ToolError> {
    installed_connection_failure_output(
        &InstalledConnectionFailure {
            source: extension.source,
            package_digest: extension.installed.digest(),
            connection,
            server,
            notices: extension.installed.notices(),
            skills: extension.skills,
        },
        error,
    )
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
