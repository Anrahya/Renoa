use renoa_agent::{ContentBlock, ToolError, ToolOutput};
use serde_json::{Value, json};

use crate::mcp::{
    AlphaMcpTool, McpAdapterError, McpCredentialError, McpOutcomeCertainty, McpToolReference,
    call::McpCallFailure,
};

pub(super) fn execution_details(
    reference: &McpToolReference,
    selected: &AlphaMcpTool,
    structured_content: Option<&Value>,
) -> Value {
    json!({
        "mcp": {
            "reference": reference.to_string(),
            "integration_id": selected.integration_id(),
            "connection_id": selected.connection_id(),
            "tool_name": selected.tool().name(),
            "structured_content": structured_content
        }
    })
}

pub(super) fn map_failure(
    reference: &McpToolReference,
    selected: &AlphaMcpTool,
    failure: McpCallFailure,
) -> Result<ToolOutput, ToolError> {
    let (source, certainty, partial_changes_possible) = failure.into_parts();
    let target = format!("{}/{}", selected.connection_id(), selected.tool().name());
    if certainty == McpOutcomeCertainty::Unknown {
        return Err(ToolError::outcome_unknown(format!(
            "MCP tool `{target}` has an unknown remote outcome: {source}"
        )));
    }
    if let McpAdapterError::Remote(remote) = &source {
        return Ok(ToolOutput {
            content: vec![ContentBlock::text(format!(
                "MCP tool `{target}` failed: {}",
                remote.message()
            ))],
            details: Some(json!({
                "mcp": {
                    "reference": reference.to_string(),
                    "integration_id": selected.integration_id(),
                    "connection_id": selected.connection_id(),
                    "tool_name": selected.tool().name(),
                    "failure": {
                        "kind": remote.kind().as_str(),
                        "certainty": remote.certainty().as_str(),
                        "partial_changes_possible": remote.partial_changes_possible(),
                        "diagnostic": {
                            "code": remote.diagnostic_code(),
                            "http_status": remote.diagnostic_http_status(),
                            "detail": remote.diagnostic_detail()
                        }
                    }
                }
            })),
            is_error: true,
        });
    }
    Err(definite_boundary_error(&source, partial_changes_possible))
}

pub(super) fn definite_boundary_error(
    source: &McpAdapterError,
    partial_changes_possible: bool,
) -> ToolError {
    let message = source.to_string();
    match source {
        McpAdapterError::InputLimit => ToolError::invalid_input(message),
        McpAdapterError::Timeout => ToolError::timeout(message, partial_changes_possible),
        McpAdapterError::Cancelled => ToolError::cancelled(message, partial_changes_possible),
        McpAdapterError::Credential(McpCredentialError::Cancelled) => {
            ToolError::cancelled(message, false)
        }
        McpAdapterError::Credential(McpCredentialError::Timeout) => {
            ToolError::timeout(message, false)
        }
        McpAdapterError::Credential(
            McpCredentialError::Unavailable { .. } | McpCredentialError::InvalidOutput,
        ) => ToolError::permission_denied(message),
        McpAdapterError::Credential(
            McpCredentialError::Start(_)
            | McpCredentialError::MissingPipe
            | McpCredentialError::Wait(_)
            | McpCredentialError::Cleanup(_)
            | McpCredentialError::Read { .. }
            | McpCredentialError::ReaderTask(_, _)
            | McpCredentialError::OutputLimit,
        ) => ToolError::unavailable(message),
        McpAdapterError::OutputLimit => ToolError::output_limit(message),
        McpAdapterError::Start(_) | McpAdapterError::Resolve(_) | McpAdapterError::NotFile(_) => {
            ToolError::unavailable(message)
        }
        McpAdapterError::Write(_)
        | McpAdapterError::Wait(_)
        | McpAdapterError::Read { .. }
        | McpAdapterError::ReaderTask(_, _)
        | McpAdapterError::MissingPipe(_)
        | McpAdapterError::Cleanup(_)
        | McpAdapterError::Protocol(_)
        | McpAdapterError::Encode(_) => {
            ToolError::process_failed(message, partial_changes_possible)
        }
        McpAdapterError::Remote(_) => {
            ToolError::internal("remote MCP failure was classified twice")
        }
    }
}
