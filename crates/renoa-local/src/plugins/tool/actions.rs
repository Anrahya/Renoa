use renoa_agent::{ToolError, ToolOutput, ToolUpdates};
use tokio_util::sync::CancellationToken;

use super::{
    AuthorizedOutput, ConnectedOutput, ConnectionOutput, InstalledOutput, ManageTool,
    output::{
        InstalledConnectionFailure, installed_connection_failure_output, json_output, plugin_error,
        remote_mcp_error_output,
    },
};
use crate::{
    mcp::{McpAdapterError, McpCatalogSnapshot, McpHostError},
    plugins::{
        ExtensionAddRequest, InstalledPlugin, PluginCredential, PluginError,
        manager::{
            ExtensionAddOutcome, ExtensionConnectionOutcome, ExtensionSourceReceipt,
            ProfileConnectionRequest,
        },
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
    pub(super) replace: bool,
}

pub(super) async fn connect(
    tool: &ManageTool,
    request: ConnectRequest,
    invocation: ExtensionInvocation<'_>,
) -> Result<ToolOutput, ToolError> {
    let (snapshot, status) = match tool
        .manager
        .connect_profile_operation(
            ProfileConnectionRequest {
                profile_id: &tool.profile_id,
                package_digest: &request.package_digest,
                server_id: &request.server,
                connection_id: &request.connection,
                credential: request.credential,
                replace: request.replace,
                operation_id: invocation.operation_id,
                updates: Some(invocation.updates),
            },
            invocation.cancellation.clone(),
        )
        .await
    {
        Ok(snapshot) => (snapshot, "catalog_loaded"),
        Err(PluginError::Mcp(McpHostError::Adapter(McpAdapterError::Remote(remote)))) => {
            return remote_mcp_error_output(&remote);
        }
        Err(error) => return Err(plugin_error(error, true)),
    };
    json_output(&ConnectionOutput {
        status,
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
        .authorize_profile(
            &tool.profile_id,
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
        .add_to_profile(
            &tool.profile_id,
            request,
            invocation.operation_id,
            Some(invocation.updates),
            invocation.cancellation.clone(),
        )
        .await
    {
        Ok(added) => added,
        Err(error) => return Err(plugin_error(error, true)),
    };
    render_added(added)
}

struct AddedExtensionView<'a> {
    source: &'static str,
    installed: &'a InstalledPlugin,
    skills: &'a SkillComponentReport,
}

fn render_added(added: ExtensionAddOutcome) -> Result<ToolOutput, ToolError> {
    let source = source_output(&added.source);
    let output = AddedExtensionView {
        source,
        installed: &added.installed,
        skills: &added.skills,
    };
    match added.connection {
        ExtensionConnectionOutcome::NotRequested => installed_output(&output),
        ExtensionConnectionOutcome::Connected {
            id,
            server,
            snapshot,
        } => connected_output(&output, &id, &server, &snapshot, "catalog_loaded"),
        ExtensionConnectionOutcome::Failed { id, server, error } => {
            failed_output(&output, id.as_deref(), server.as_deref(), error)
        }
    }
}

fn installed_output(extension: &AddedExtensionView<'_>) -> Result<ToolOutput, ToolError> {
    json_output(&InstalledOutput {
        status: "installed",
        source: extension.source,
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
    status: &'static str,
) -> Result<ToolOutput, ToolError> {
    json_output(&ConnectedOutput {
        status,
        source: extension.source,
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

fn failed_output(
    extension: &AddedExtensionView<'_>,
    connection: Option<&str>,
    server: Option<&str>,
    error: PluginError,
) -> Result<ToolOutput, ToolError> {
    installed_failure(extension, connection, server, error)
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

fn source_output(receipt: &ExtensionSourceReceipt) -> &'static str {
    match receipt {
        ExtensionSourceReceipt::Mcp => "mcp",
        ExtensionSourceReceipt::Package => "package",
    }
}
