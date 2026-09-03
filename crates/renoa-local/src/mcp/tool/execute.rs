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
            content: vec![ContentBlock::text(remote_failure_message(
                &target,
                selected.connection_id(),
                remote,
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
                    "required_scope": match source {
                        McpAdapterError::Remote(remote) => remote.required_oauth_scope(),
                        _ => None,
                    },
                    "detail": detail
                }
            }
        }
    })
}

fn remote_failure_message(
    target: &str,
    connection: &str,
    remote: &crate::mcp::McpRemoteFailure,
) -> String {
    if remote.diagnostic_code() != Some("oauth_insufficient_scope") {
        return format!("MCP tool `{target}` failed: {}", remote.message());
    }
    let Some(scope) = remote.required_oauth_scope() else {
        return format!(
            "MCP tool `{target}` needs additional OAuth permission, but the server did not return a usable required_scope. Renoa did not retry the tool. Do not guess a provider scope; check the service's official documentation and report the missing scope challenge."
        );
    };
    let authorize = json!({
        "action": "authorize",
        "connection": connection,
        "required_scope": scope,
    });
    format!(
        "MCP tool `{target}` needs additional OAuth permission. Renoa did not retry the tool. Call `extension_manage` with exactly {authorize}. After authorization succeeds, explicitly retry this tool once."
    )
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
        McpAdapterError::Credential(
            McpCredentialError::Timeout(_) | McpCredentialError::SetupExpired,
        ) => ToolError::timeout(message, false),
        McpAdapterError::Credential(
            McpCredentialError::Unavailable { .. }
            | McpCredentialError::InvalidOutput(_)
            | McpCredentialError::SetupInvalid,
        ) => ToolError::permission_denied(message),
        McpAdapterError::Credential(
            McpCredentialError::PrivateStore(_)
            | McpCredentialError::SetupUnavailable(_)
            | McpCredentialError::Start { .. }
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

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::remote_failure_message;
    use crate::mcp::McpRemoteFailure;

    #[test]
    fn scope_failure_tells_the_agent_the_only_valid_recovery_call() {
        let failure: McpRemoteFailure = serde_json::from_value(json!({
            "kind": "protocol",
            "certainty": "definite",
            "message": "additional authorization required",
            "partial_changes_possible": false,
            "diagnostic": {
                "code": "oauth_insufficient_scope",
                "http_status": 403,
                "required_scope": "tweet.write users.read",
                "detail": "write access is required"
            }
        }))
        .expect("decode scope failure");

        let message = remote_failure_message("x-api/create_post", "x-api", &failure);

        assert!(message.contains("Renoa did not retry the tool"));
        assert!(message.contains(
            r#"{"action":"authorize","connection":"x-api","required_scope":"tweet.write users.read"}"#
        ));
        assert!(message.contains("explicitly retry this tool once"));
    }
}
