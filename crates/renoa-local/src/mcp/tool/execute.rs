use renoa_agent::{ContentBlock, ToolError, ToolOutput};
use serde_json::{Value, json};

use crate::mcp::{
    McpAdapterError, McpCredentialError, McpOutcomeCertainty, McpToolReference, ResolvedMcpTool,
    call::McpCallFailure,
};

pub(super) fn execution_details(
    reference: &McpToolReference,
    selected: &ResolvedMcpTool,
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
    selected: &ResolvedMcpTool,
    failure: McpCallFailure,
) -> Result<ToolOutput, ToolError> {
    let (source, certainty, partial_changes_possible) = failure.into_parts();
    let target = format!("{}/{}", selected.connection_id(), selected.tool().name());
    if certainty == McpOutcomeCertainty::Unknown {
        return Ok(ToolOutput {
            content: vec![ContentBlock::text(format!(
                "Renoa received no final response from MCP tool `{target}`. The call may or may not have succeeded. Renoa did not replay it. Do not retry blindly; explain the uncertainty or verify state with a safe read before deciding what to do. Boundary error: {source}"
            ))],
            details: Some(failure_details(
                reference,
                selected,
                &source,
                certainty,
                partial_changes_possible,
            )),
            is_error: true,
        });
    }
    if let McpAdapterError::Remote(remote) = &source {
        return Ok(ToolOutput {
            content: vec![ContentBlock::text(format!(
                "MCP tool `{target}` failed: {}",
                remote.message()
            ))],
            details: Some(failure_details(
                reference,
                selected,
                &source,
                certainty,
                partial_changes_possible,
            )),
            is_error: true,
        });
    }
    Err(definite_boundary_error(&source, partial_changes_possible))
}

fn failure_details(
    reference: &McpToolReference,
    selected: &ResolvedMcpTool,
    source: &McpAdapterError,
    certainty: McpOutcomeCertainty,
    partial_changes_possible: bool,
) -> Value {
    let (kind, code, http_status, detail) = match source {
        McpAdapterError::Remote(remote) => (
            remote.kind().as_str(),
            remote.diagnostic_code(),
            remote.diagnostic_http_status(),
            remote.diagnostic_detail(),
        ),
        _ => (
            "adapter_boundary",
            None,
            None,
            "No terminal adapter response.",
        ),
    };
    json!({
        "mcp": {
            "reference": reference.to_string(),
            "integration_id": selected.integration_id(),
            "connection_id": selected.connection_id(),
            "tool_name": selected.tool().name(),
            "failure": {
                "kind": kind,
                "certainty": certainty.as_str(),
                "partial_changes_possible": partial_changes_possible,
                "diagnostic": {
                    "code": code,
                    "http_status": http_status,
                    "detail": detail
                }
            }
        }
    })
}

pub(crate) fn definite_boundary_error(
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
        McpAdapterError::Credential(McpCredentialError::Timeout(_)) => {
            ToolError::timeout(message, false)
        }
        McpAdapterError::Credential(
            McpCredentialError::Unavailable { .. } | McpCredentialError::InvalidOutput(_),
        ) => ToolError::permission_denied(message),
        McpAdapterError::Credential(
            McpCredentialError::Start { .. }
            | McpCredentialError::Write { .. }
            | McpCredentialError::MissingPipe(_)
            | McpCredentialError::Wait { .. }
            | McpCredentialError::Cleanup { .. }
            | McpCredentialError::Read { .. }
            | McpCredentialError::ReaderTask { .. }
            | McpCredentialError::OutputLimit(_),
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
