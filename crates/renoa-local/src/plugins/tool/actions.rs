use super::{
    AuthorizedOutput, ConnectedOutput, ConnectionOutput, DisconnectedOutput, EnabledOutput,
    InstalledOutput,
    output::{InstalledConnectionFailure, installed_connection_failure_output, json_output},
};
use crate::mcp::McpCatalogSnapshot;
use crate::plugins::{
    InstalledPlugin, PluginAddOutcome, PluginConnectionOutcome, PluginError, PluginSourceReceipt,
    api::PluginOutcome,
};
use crate::skills::SkillComponentReport;
use renoa_agent::{ToolError, ToolOutput};

pub(super) fn render(outcome: PluginOutcome) -> Result<ToolOutput, ToolError> {
    match outcome {
        PluginOutcome::Inspected(inspection) => json_output(&inspection),
        PluginOutcome::Installed(installed) => json_output(&installed),
        PluginOutcome::Listed(page) => json_output(&page),
        PluginOutcome::Activation(activation) => json_output(&activation),
        PluginOutcome::HostActivation(activation) => json_output(&activation),
        PluginOutcome::Added(added) => render_added(*added),
        PluginOutcome::Connected {
            package_digest,
            server,
            connection,
            snapshot,
        } => json_output(&ConnectionOutput {
            status: "catalog_loaded",
            package_digest,
            server,
            connection,
            catalog_digest: snapshot.digest().to_owned(),
            tools: snapshot.tools().len(),
            rejected_tools: snapshot.rejected_tools().len(),
        }),
        PluginOutcome::Authorized {
            connection,
            snapshot,
        } => json_output(&AuthorizedOutput {
            status: "authorized",
            connection,
            catalog_digest: snapshot.digest().to_owned(),
            tools: snapshot.tools().len(),
            rejected_tools: snapshot.rejected_tools().len(),
        }),
        PluginOutcome::Disconnected {
            connection,
            catalog_retained,
        } => json_output(&DisconnectedOutput {
            status: "disconnected",
            connection,
            catalog_retained,
            enabled_for_agent: false,
        }),
        PluginOutcome::Enabled { connection } => json_output(&EnabledOutput {
            status: "enabled",
            connection,
            catalog_retained: true,
            enabled_for_agent: true,
        }),
    }
}

struct AddedExtensionView<'a> {
    source: &'static str,
    installed: &'a InstalledPlugin,
    skills: &'a SkillComponentReport,
    activation: &'a crate::plugins::PluginActivation,
}

fn render_added(mut added: PluginAddOutcome) -> Result<ToolOutput, ToolError> {
    added.activation.skills = None;
    let source = source_output(&added.source);
    let output = AddedExtensionView {
        source,
        installed: &added.installed,
        skills: &added.skills,
        activation: &added.activation,
    };
    match added.connection {
        PluginConnectionOutcome::NotRequested => installed_output(&output),
        PluginConnectionOutcome::Connected {
            id,
            server,
            snapshot,
        } => connected_output(&output, &id, &server, &snapshot, "catalog_loaded"),
        PluginConnectionOutcome::Failed { id, server, error } => {
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
        activation: extension.activation,
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
        activation: extension.activation,
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
            activation: extension.activation,
        },
        error,
    )
}

fn source_output(receipt: &PluginSourceReceipt) -> &'static str {
    match receipt {
        PluginSourceReceipt::Mcp => "mcp",
        PluginSourceReceipt::Package => "package",
        PluginSourceReceipt::Installed => "installed",
        PluginSourceReceipt::Skill => "skill",
        PluginSourceReceipt::Github => "github",
    }
}
