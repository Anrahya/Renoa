use std::{path::PathBuf, sync::Arc};

use renoa_agent::{
    BoxFuture, ContentBlock, Tool, ToolCall, ToolError, ToolOutput, ToolSpec, ToolUpdates,
};
use renoa_agent_loop::AgentToolBinding;
use renoa_kernel::EffectRecovery;
use serde::Serialize;
use serde_json::{Value, json};
use tokio_util::sync::CancellationToken;

use super::{
    AlphaMcpTool, McpAdapterError, McpConnectionAuth, McpCredentialError, McpCredentialResolver,
    McpHostError, McpOutcomeCertainty,
    call::{CALL_BOUNDARY_REVISION, McpCallFailure, call_tool},
    hex_sha256,
};

pub(crate) fn alpha_tool_binding(
    adapter: PathBuf,
    credentials: McpCredentialResolver,
    selected: AlphaMcpTool,
) -> Result<AgentToolBinding, McpHostError> {
    let revision = binding_revision(&selected)?;
    let tool: Arc<dyn Tool> = Arc::new(McpTool::new(adapter, credentials, selected));
    Ok(AgentToolBinding::new(
        revision,
        tool,
        EffectRecovery::NeverReplay,
    ))
}

struct McpTool {
    adapter: PathBuf,
    selected: AlphaMcpTool,
    credentials: McpCredentialResolver,
    spec: ToolSpec,
}

impl McpTool {
    fn new(adapter: PathBuf, credentials: McpCredentialResolver, selected: AlphaMcpTool) -> Self {
        let spec = ToolSpec {
            name: selected.tool().name().to_owned(),
            description: selected.tool().description().to_owned(),
            input_schema: selected.tool().model_input_schema().clone(),
        };
        Self {
            adapter,
            selected,
            credentials,
            spec,
        }
    }
}

impl Tool for McpTool {
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
            if call.name != self.spec.name {
                return Err(ToolError::invalid_input(format!(
                    "MCP binding `{}` cannot execute call for `{}`",
                    self.spec.name, call.name
                )));
            }
            if !call.arguments.is_object() {
                return Err(ToolError::invalid_input(
                    "MCP tool arguments must be a JSON object",
                ));
            }
            let authorization = self
                .credentials
                .resolve(self.selected.auth(), cancellation.clone())
                .await
                .map_err(McpAdapterError::from)
                .map_err(|error| definite_boundary_error(&error, false))?;
            match call_tool(
                &self.adapter,
                &self.selected,
                authorization.as_ref(),
                &call.arguments,
                cancellation,
            )
            .await
            {
                Ok(result) => Ok(ToolOutput {
                    content: result.content,
                    details: result.details,
                    is_error: result.is_error,
                }),
                Err(error) => map_failure(&self.spec.name, error),
            }
        })
    }
}

fn map_failure(tool_name: &str, failure: McpCallFailure) -> Result<ToolOutput, ToolError> {
    let (source, certainty, partial_changes_possible) = failure.into_parts();
    if certainty == McpOutcomeCertainty::Unknown {
        return Err(ToolError::outcome_unknown(format!(
            "MCP tool `{tool_name}` has an unknown remote outcome: {source}"
        )));
    }
    if let McpAdapterError::Remote(remote) = &source {
        return Ok(ToolOutput {
            content: vec![ContentBlock::text(format!(
                "MCP tool `{tool_name}` failed: {}",
                remote.message()
            ))],
            details: Some(json!({
                "mcp_failure": {
                    "kind": remote.kind().as_str(),
                    "certainty": remote.certainty().as_str(),
                    "partial_changes_possible": remote.partial_changes_possible(),
                    "diagnostic": {
                        "code": remote.diagnostic_code(),
                        "http_status": remote.diagnostic_http_status(),
                        "detail": remote.diagnostic_detail()
                    }
                }
            })),
            is_error: true,
        });
    }
    Err(definite_boundary_error(&source, partial_changes_possible))
}

fn definite_boundary_error(source: &McpAdapterError, partial_changes_possible: bool) -> ToolError {
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

fn binding_revision(selected: &AlphaMcpTool) -> Result<String, McpHostError> {
    #[derive(Serialize)]
    struct FrozenBinding<'a> {
        integration_id: &'a str,
        connection_id: &'a str,
        endpoint: &'a str,
        protocol_version: &'a str,
        adapter_revision: &'a str,
        auth: &'a McpConnectionAuth,
        host_call_boundary_revision: &'static str,
        tool_name: &'a str,
        input_schema: &'a Value,
        model_input_schema: &'a Value,
        output_schema: Option<&'a Value>,
        recovery: &'static str,
        result_projection: &'static str,
    }

    let encoded = serde_json::to_vec(&FrozenBinding {
        integration_id: selected.integration_id(),
        connection_id: selected.connection_id(),
        endpoint: selected.endpoint(),
        protocol_version: selected.protocol_version(),
        adapter_revision: selected.adapter_revision(),
        auth: selected.auth(),
        host_call_boundary_revision: CALL_BOUNDARY_REVISION,
        tool_name: selected.tool().name(),
        input_schema: selected.tool().input_schema(),
        model_input_schema: selected.tool().model_input_schema(),
        output_schema: selected.tool().output_schema(),
        recovery: "never_replay",
        result_projection: "ordered-text-image-plus-structured-v1",
    })?;
    Ok(format!("renoa-mcp-tool/v1/{}", hex_sha256(&encoded)))
}

#[cfg(test)]
mod tests {
    use super::{binding_revision, definite_boundary_error};
    use crate::mcp::{
        AlphaMcpTool, MCP_ADAPTER_REVISION, MCP_PROTOCOL_VERSION, McpAdapterError, McpCatalogTool,
        McpConnectionAuth,
    };

    #[test]
    fn a_pre_dispatch_input_limit_is_model_visible_and_has_no_partial_change() {
        let error = definite_boundary_error(&McpAdapterError::InputLimit, false);

        assert!(!error.outcome_is_unknown());
        assert!(!error.partial_changes_possible());
    }

    #[test]
    fn execution_identity_changes_with_endpoint_schema_and_adapter_behavior() {
        let base = selected("https://example.com/mcp", "string", MCP_ADAPTER_REVISION);
        let changed_endpoint = selected(
            "https://other.example.com/mcp",
            "string",
            MCP_ADAPTER_REVISION,
        );
        let changed_schema = selected("https://example.com/mcp", "integer", MCP_ADAPTER_REVISION);
        let changed_adapter = selected("https://example.com/mcp", "string", "next-adapter");
        let revision = binding_revision(&base).expect("encode base binding");

        assert_ne!(
            revision,
            binding_revision(&changed_endpoint).expect("encode endpoint binding")
        );
        assert_ne!(
            revision,
            binding_revision(&changed_schema).expect("encode schema binding")
        );
        assert_ne!(
            revision,
            binding_revision(&changed_adapter).expect("encode adapter binding")
        );
    }

    fn selected(endpoint: &str, value_type: &str, adapter_revision: &str) -> AlphaMcpTool {
        let schema = serde_json::json!({
            "type": "object",
            "properties": {"value": {"type": value_type}},
            "required": ["value"]
        });
        AlphaMcpTool {
            integration_id: "fixture".to_owned(),
            connection_id: "primary".to_owned(),
            endpoint: endpoint.to_owned(),
            protocol_version: MCP_PROTOCOL_VERSION.to_owned(),
            adapter_revision: adapter_revision.to_owned(),
            auth: McpConnectionAuth::None,
            tool: McpCatalogTool {
                name: "echo".to_owned(),
                description: "Echo a value.".to_owned(),
                input_schema: schema.clone(),
                model_input_schema: schema,
                output_schema: None,
            },
        }
    }
}
