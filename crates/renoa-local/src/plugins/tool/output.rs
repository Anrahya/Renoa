use renoa_agent::{ContentBlock, ToolError, ToolErrorCode, ToolOutput};
use serde::Serialize;
use serde_json::json;

use super::super::{
    PluginError,
    catalog::{CatalogError, CatalogFailure, CatalogFailureKind},
};
use crate::{
    mcp::{
        McpAdapterError, McpFailureKind, McpHostError, McpOutcomeCertainty, McpRemoteFailure,
        adapter_tool_error,
    },
    output::MAX_TOOL_OUTPUT_BYTES,
};

pub(super) fn json_output(value: &impl Serialize) -> Result<ToolOutput, ToolError> {
    encoded_output(value, None, false)
}

fn encoded_output(
    value: &impl Serialize,
    details: Option<serde_json::Value>,
    is_error: bool,
) -> Result<ToolOutput, ToolError> {
    let content = serde_json::to_string(value).map_err(|error| {
        ToolError::internal(format!("extension result could not be encoded: {error}"))
    })?;
    if content.len() > MAX_TOOL_OUTPUT_BYTES {
        return Err(ToolError::output_limit(format!(
            "extension result exceeds the {MAX_TOOL_OUTPUT_BYTES}-byte tool output boundary"
        )));
    }
    Ok(ToolOutput {
        content: vec![ContentBlock::text(content)],
        details,
        is_error,
    })
}

pub(super) fn remote_mcp_error_output(remote: &McpRemoteFailure) -> Result<ToolOutput, ToolError> {
    let (model, details) = remote_mcp_error_values(remote);
    encoded_output(&model, Some(details), true)
}

pub(super) struct InstalledConnectionFailure<'a> {
    pub(super) source: &'static str,
    pub(super) package_digest: &'a str,
    pub(super) connection: Option<&'a str>,
    pub(super) server: Option<&'a str>,
    pub(super) notices: &'a [crate::plugins::PluginNotice],
    pub(super) skills: &'a crate::skills::SkillComponentReport,
}

pub(super) fn installed_connection_failure_output(
    context: &InstalledConnectionFailure<'_>,
    error: PluginError,
) -> Result<ToolOutput, ToolError> {
    let (mut model, mut details) = match error {
        PluginError::Mcp(McpHostError::Adapter(McpAdapterError::Remote(remote))) => {
            remote_mcp_error_values(&remote)
        }
        error => {
            let error = plugin_error(error, true);
            let retryable = matches!(
                error.code(),
                ToolErrorCode::Timeout | ToolErrorCode::Unavailable | ToolErrorCode::Io
            );
            (
                json!({
                    "code": error.code(),
                    "message": error.to_string(),
                    "retryable": retryable,
                    "next_action": "The package remains installed. Correct the connection requirement, then retry the connection without reinstalling the package."
                }),
                json!({
                    "error": {
                        "code": error.code(),
                        "partial_changes_possible": true
                    }
                }),
            )
        }
    };
    attach_installation(&mut model, context)?;
    attach_installation(&mut details, context)?;
    encoded_output(&model, Some(details), true)
}

fn remote_mcp_error_values(remote: &McpRemoteFailure) -> (serde_json::Value, serde_json::Value) {
    if remote.certainty() == McpOutcomeCertainty::Unknown {
        let model = json!({
            "code": "mcp_outcome_unknown",
            "message": format!(
                "Renoa received no final response while checking the MCP connection. The request may or may not have succeeded. Renoa did not replay it. {}",
                remote.message()
            ),
            "retryable": false,
            "next_action": "Do not retry blindly. Explain the uncertainty or verify current state with a safe read before deciding what to do."
        });
        let details = json!({
            "mcp": {
                "failure": {
                    "kind": remote.kind().as_str(),
                    "certainty": remote.certainty().as_str(),
                    "partial_changes_possible": remote.partial_changes_possible(),
                    "diagnostic": {
                        "code": remote.diagnostic_code(),
                        "http_status": remote.diagnostic_http_status(),
                        "detail": remote.diagnostic_detail(),
                    }
                }
            }
        });
        return (model, details);
    }
    let retryable = matches!(
        remote.kind(),
        McpFailureKind::Timeout | McpFailureKind::Unavailable | McpFailureKind::Transport
    );
    let next_action = match remote.kind() {
        McpFailureKind::Timeout | McpFailureKind::Unavailable | McpFailureKind::Transport => {
            "Check that the endpoint is reachable, then retry once."
        }
        McpFailureKind::Cancelled => "Retry only if the user still wants this extension.",
        McpFailureKind::Internal => "Stop guessing and report this adapter failure to the user.",
        McpFailureKind::InvalidRequest
        | McpFailureKind::InvalidEndpoint
        | McpFailureKind::IncompatibleProtocol
        | McpFailureKind::Protocol
        | McpFailureKind::ResourceLimit
        | McpFailureKind::UnsupportedResult
        | McpFailureKind::InvalidResult => {
            "Do not retry unchanged. Correct the connection or tell the user why it is incompatible."
        }
    };
    let model = json!({
        "code": format!("mcp_{}", remote.kind().as_str()),
        "message": remote.message(),
        "retryable": retryable,
        "next_action": next_action,
    });
    let details = json!({
        "mcp": {
            "failure": {
                "kind": remote.kind().as_str(),
                "certainty": remote.certainty().as_str(),
                "partial_changes_possible": remote.partial_changes_possible(),
                "diagnostic": {
                    "code": remote.diagnostic_code(),
                    "http_status": remote.diagnostic_http_status(),
                    "detail": remote.diagnostic_detail(),
                }
            }
        }
    });
    (model, details)
}

fn attach_installation(
    value: &mut serde_json::Value,
    context: &InstalledConnectionFailure<'_>,
) -> Result<(), ToolError> {
    let Some(object) = value.as_object_mut() else {
        return Err(ToolError::internal(
            "extension connection failure was not encoded as an object",
        ));
    };
    object.insert(
        "status".to_owned(),
        serde_json::Value::String("installed_connection_failed".to_owned()),
    );
    object.insert(
        "source".to_owned(),
        serde_json::Value::String(context.source.to_owned()),
    );
    object.insert(
        "package_digest".to_owned(),
        serde_json::Value::String(context.package_digest.to_owned()),
    );
    if let Some(connection) = context.connection {
        object.insert(
            "connection".to_owned(),
            serde_json::Value::String(connection.to_owned()),
        );
    }
    if let Some(server) = context.server {
        object.insert(
            "server".to_owned(),
            serde_json::Value::String(server.to_owned()),
        );
    }
    object.insert(
        "notices".to_owned(),
        serde_json::to_value(context.notices).map_err(|error| {
            ToolError::internal(format!("extension notices could not be encoded: {error}"))
        })?,
    );
    object.insert(
        "skills".to_owned(),
        serde_json::to_value(context.skills).map_err(|error| {
            ToolError::internal(format!("extension skills could not be encoded: {error}"))
        })?,
    );
    Ok(())
}

pub(super) fn catalog_failure_output(failure: &CatalogFailure) -> Result<ToolOutput, ToolError> {
    let retryable = matches!(failure.kind(), CatalogFailureKind::Unavailable);
    let next_action = if retryable {
        "Retry once later. If discovery remains unavailable, use web research or a local Agent Plugin package."
    } else {
        "Do not retry unchanged. Search again if the candidate changed; otherwise use web research or a local Agent Plugin package."
    };
    let model = json!({
        "code": format!("integration_catalog_{}", failure.kind().as_str()),
        "message": failure.message(),
        "retryable": retryable,
        "next_action": next_action,
    });
    let diagnostic = failure.diagnostic();
    let details = json!({
        "integration_catalog": {
            "failure": {
                "kind": failure.kind().as_str(),
                "diagnostic": {
                    "code": diagnostic.and_then(super::super::catalog::CatalogDiagnostic::code),
                    "http_status": diagnostic.and_then(super::super::catalog::CatalogDiagnostic::http_status),
                    "detail": diagnostic.and_then(super::super::catalog::CatalogDiagnostic::detail),
                }
            }
        }
    });
    encoded_output(&model, Some(details), true)
}

pub(super) fn plugin_error(error: PluginError, partial_changes_possible: bool) -> ToolError {
    let message = error.to_string();
    match error {
        PluginError::Invalid(_)
        | PluginError::Mcp(McpHostError::Invalid(_))
        | PluginError::Skill(crate::skills::SkillError::Invalid(_)) => {
            ToolError::invalid_input(message)
        }
        PluginError::Conflict(_)
        | PluginError::Mcp(McpHostError::Conflict(_))
        | PluginError::Skill(crate::skills::SkillError::Conflict(_)) => {
            ToolError::conflict(message)
        }
        PluginError::NotFound(_)
        | PluginError::Mcp(McpHostError::NotFound(_))
        | PluginError::Skill(crate::skills::SkillError::NotFound(_)) => {
            ToolError::not_found(message)
        }
        PluginError::Io { .. } | PluginError::Skill(crate::skills::SkillError::Io { .. }) => {
            ToolError::io(message, partial_changes_possible)
        }
        PluginError::Unavailable(_)
        | PluginError::Database(_)
        | PluginError::Json(_)
        | PluginError::HostCatalog(_)
        | PluginError::Background(_)
        | PluginError::Skill(
            crate::skills::SkillError::Database(_) | crate::skills::SkillError::HostCatalog(_),
        )
        | PluginError::Mcp(
            McpHostError::Io(_)
            | McpHostError::Database(_)
            | McpHostError::HostCatalog(_)
            | McpHostError::Json(_),
        )
        | PluginError::Catalog(
            CatalogError::Resolve(_)
            | CatalogError::NotFile(_)
            | CatalogError::Start(_)
            | CatalogError::MissingPipe(_)
            | CatalogError::Write(_)
            | CatalogError::Wait(_)
            | CatalogError::Timeout
            | CatalogError::OutputLimit
            | CatalogError::Protocol(_)
            | CatalogError::Cleanup(_)
            | CatalogError::Encode(_)
            | CatalogError::Reader(_),
        ) => ToolError::unavailable(message),
        PluginError::Mcp(McpHostError::Adapter(McpAdapterError::Remote(remote))) => {
            ToolError::process_failed(remote.to_string(), partial_changes_possible)
        }
        PluginError::Mcp(McpHostError::Adapter(error)) => {
            adapter_tool_error(&error, partial_changes_possible)
        }
        PluginError::Catalog(CatalogError::Cancelled) => {
            ToolError::cancelled(message, partial_changes_possible)
        }
        PluginError::Catalog(CatalogError::Remote(failure)) => {
            ToolError::process_failed(failure.to_string(), partial_changes_possible)
        }
    }
}
