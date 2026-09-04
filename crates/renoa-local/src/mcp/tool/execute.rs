use renoa_agent::{ContentBlock, ToolError, ToolOutput};
use serde_json::{Value, json};

use crate::mcp::{
    McpAdapterError, McpCredentialError, McpHostError, McpOAuthError, McpOutcomeCertainty,
    McpToolReference, ResolvedMcpTool, call::McpCallFailure,
};

pub(super) fn authorization_failure(
    reference: &McpToolReference,
    selected: &ResolvedMcpTool,
    error: McpHostError,
) -> Result<ToolOutput, ToolError> {
    match error {
        McpHostError::Adapter(McpAdapterError::Remote(remote)) => {
            Ok(remote_authorization_failure(reference, selected, &remote))
        }
        error @ McpHostError::OAuth(McpOAuthError::OutcomeUnknown { .. }) => {
            Ok(unknown_authorization_failure(reference, selected, &error))
        }
        error => Err(super::host_error(error)),
    }
}

fn remote_authorization_failure(
    reference: &McpToolReference,
    selected: &ResolvedMcpTool,
    remote: &crate::mcp::McpRemoteFailure,
) -> ToolOutput {
    let target = format!("{}/{}", selected.connection_id(), selected.tool().name());
    let failure = json!({
        "kind": remote.kind().as_str(),
        "certainty": remote.certainty().as_str(),
        "partial_changes_possible": remote.partial_changes_possible(),
        "diagnostic": {
            "code": remote.diagnostic_code(),
            "http_status": remote.diagnostic_http_status(),
            "detail": remote.diagnostic_detail(),
        }
    });
    let recovery = if remote.diagnostic_code() == Some("oauth_refresh_token_missing") {
        format!(
            " Call `extension_manage` with exactly {} before retrying the MCP tool.",
            restart_authorization(selected)
        )
    } else {
        String::new()
    };
    let message = if remote.certainty() == McpOutcomeCertainty::Unknown {
        format!(
            "OAuth credential handling for MCP connection '{}' returned no final response. MCP tool `{target}` was not dispatched and was not retried. The credential exchange may or may not have completed. {} Diagnostic: {}{recovery}",
            selected.connection_id(),
            remote.message(),
            failure["diagnostic"],
        )
    } else {
        format!(
            "MCP authorization for connection '{}' failed before tool `{target}` was dispatched: {} Diagnostic: {}{recovery}",
            selected.connection_id(),
            remote.message(),
            failure["diagnostic"],
        )
    };
    ToolOutput {
        content: vec![ContentBlock::text(message)],
        details: Some(authorization_details(reference, selected, &failure)),
        is_error: true,
    }
}

fn unknown_authorization_failure(
    reference: &McpToolReference,
    selected: &ResolvedMcpTool,
    error: &McpHostError,
) -> ToolOutput {
    let target = format!("{}/{}", selected.connection_id(), selected.tool().name());
    let restart = restart_authorization(selected);
    let message = format!(
        "{error} The credential exchange may or may not have completed. MCP tool `{target}` was not dispatched and was not retried. Call `extension_manage` with exactly {restart} before calling the MCP tool again."
    );
    ToolOutput {
        content: vec![ContentBlock::text(message.clone())],
        details: Some(authorization_details(
            reference,
            selected,
            &json!({
                "kind": "oauth",
                "certainty": "unknown",
                "partial_changes_possible": true,
                "diagnostic": {
                    "code": "oauth_outcome_unknown",
                    "http_status": null,
                    "detail": message,
                }
            }),
        )),
        is_error: true,
    }
}

fn restart_authorization(selected: &ResolvedMcpTool) -> Value {
    json!({
        "action": "authorize",
        "connection": selected.connection_id(),
        "restart": true,
    })
}

fn authorization_details(
    reference: &McpToolReference,
    selected: &ResolvedMcpTool,
    failure: &Value,
) -> Value {
    json!({
        "mcp": {
            "reference": reference.to_string(),
            "integration_id": selected.integration_id(),
            "connection_id": selected.connection_id(),
            "tool_name": selected.tool().name(),
            "tool_dispatched": false,
            "failure": failure,
        }
    })
}

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
        McpAdapterError::Remote(remote) if remote.certainty() == McpOutcomeCertainty::Unknown => {
            ToolError::outcome_unknown(remote.to_string())
        }
        McpAdapterError::Remote(remote) => {
            ToolError::process_failed(remote.to_string(), remote.partial_changes_possible())
        }
    }
}

#[cfg(test)]
mod tests {
    use renoa_agent::ContentBlock;
    use serde_json::json;

    use super::{authorization_failure, remote_failure_message};
    use crate::mcp::{
        McpAdapterError, McpCatalogTool, McpConnectionAuth, McpHostError, McpOAuthRegistration,
        McpRemoteFailure, McpRequestHeaders, McpToolReference, ResolvedMcpTool,
    };

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

    #[test]
    fn pre_dispatch_oauth_failure_keeps_the_remote_diagnostic() {
        let failure: McpRemoteFailure = serde_json::from_value(json!({
            "kind": "protocol",
            "certainty": "definite",
            "message": "Stored OAuth authorization cannot be refreshed.",
            "partial_changes_possible": false,
            "diagnostic": {
                "code": "oauth_refresh_token_missing",
                "http_status": 401,
                "detail": "The authorization server did not issue a refresh token."
            }
        }))
        .expect("decode OAuth failure");
        let selected = selected_tool();
        let reference = tool_reference();

        let output = authorization_failure(
            &reference,
            &selected,
            McpHostError::Adapter(McpAdapterError::Remote(failure)),
        )
        .expect("pre-dispatch remote failure is model-visible");

        assert!(output.is_error);
        let ContentBlock::Text { text } = &output.content[0] else {
            panic!("authorization failure must be text")
        };
        assert!(text.contains("Stored OAuth authorization cannot be refreshed."));
        assert!(text.contains("The authorization server did not issue a refresh token."));
        assert!(text.contains(r#"{"action":"authorize","connection":"drive","restart":true}"#));
        assert!(!text.contains("classified twice"));
        let details = output.details.expect("structured authorization details");
        assert_eq!(details["mcp"]["tool_dispatched"], false);
        assert_eq!(
            details["mcp"]["failure"]["diagnostic"]["code"],
            "oauth_refresh_token_missing"
        );
        assert_eq!(details["mcp"]["failure"]["diagnostic"]["http_status"], 401);
    }

    #[test]
    fn unknown_oauth_exchange_does_not_claim_the_mcp_tool_ran() {
        let selected = selected_tool();
        let reference = tool_reference();

        let output = authorization_failure(
            &reference,
            &selected,
            McpHostError::OAuth(crate::mcp::McpOAuthError::OutcomeUnknown {
                connection: "drive".to_owned(),
                detail: "token endpoint closed the connection".to_owned(),
            }),
        )
        .expect("unknown credential exchange is reported without dispatching the MCP tool");

        assert!(output.is_error);
        let ContentBlock::Text { text } = &output.content[0] else {
            panic!("authorization failure must be text")
        };
        assert!(text.contains("may or may not have completed"));
        assert!(text.contains("was not dispatched and was not retried"));
        assert!(text.contains(r#"{"action":"authorize","connection":"drive","restart":true}"#));
        let details = output.details.expect("structured authorization details");
        assert_eq!(details["mcp"]["tool_dispatched"], false);
        assert_eq!(details["mcp"]["failure"]["certainty"], "unknown");
        assert_eq!(
            details["mcp"]["failure"]["diagnostic"]["code"],
            "oauth_outcome_unknown"
        );
    }

    fn tool_reference() -> McpToolReference {
        "mcp:drive:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa:list_recent_files"
            .parse()
            .expect("valid tool reference")
    }

    fn selected_tool() -> ResolvedMcpTool {
        ResolvedMcpTool {
            integration_id: "google-drive".to_owned(),
            connection_id: "drive".to_owned(),
            endpoint: "https://drivemcp.googleapis.com/mcp/v1".to_owned(),
            request_headers: McpRequestHeaders::default(),
            protocol_version: "2026-07-28".to_owned(),
            adapter_revision: "mcp-client-node-v0.10.0".to_owned(),
            auth: McpConnectionAuth::oauth(
                "drive",
                "https://drivemcp.googleapis.com/mcp/v1",
                McpOAuthRegistration::dynamic(),
            )
            .expect("valid OAuth binding"),
            tool: McpCatalogTool {
                name: "list_recent_files".to_owned(),
                description: "List recent files".to_owned(),
                input_schema: json!({"type": "object"}),
                model_input_schema: json!({"type": "object"}),
                output_schema: None,
            },
        }
    }
}
