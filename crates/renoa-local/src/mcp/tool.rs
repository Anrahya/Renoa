use std::{collections::HashSet, path::PathBuf, str::FromStr as _, sync::Arc};

mod execute;
mod search;

#[cfg(test)]
mod tests;

use renoa_agent::{
    BoxFuture, ContentBlock, Tool, ToolCall, ToolError, ToolOutput, ToolSpec, ToolUpdates,
};
use renoa_agent_loop::AgentToolBinding;
use renoa_kernel::{CommandId, EffectRecovery, SessionId};
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use serde_json::{Value, json};
use tokio_util::sync::CancellationToken;

use super::{
    LOAD_OUTPUT_BYTES, LOAD_REFERENCE_LIMIT, McpAuthorizationResolver, McpCatalogStore,
    McpHostError, McpToolReference,
    call::{CALL_BOUNDARY_REVISION, call_tool},
    oauth_operation_id,
};
use execute::{authorization_failure, definite_boundary_error, execution_details, map_failure};
use renoa_kernel::AgentId;
use search::SearchTool;

pub(crate) use execute::definite_boundary_error as adapter_tool_error;

const SEARCH_TOOL: &str = crate::capabilities::TOOL_SEARCH;
const LOAD_TOOL: &str = crate::capabilities::TOOL_LOAD;
const EXECUTE_TOOL: &str = crate::capabilities::TOOL_EXECUTE;
const SEARCH_REVISION: &str = "renoa-mcp-registry-v6/search";
const LOAD_REVISION: &str = "renoa-mcp-registry-v2/load";
const EXECUTE_REVISION: &str = "renoa-mcp-registry-v2/execute";

pub(crate) fn agent_registry_bindings(
    agent_id: AgentId,
    store: McpCatalogStore,
    adapter: Option<PathBuf>,
    authorizations: McpAuthorizationResolver,
    session_id: SessionId,
    command_id: Option<CommandId>,
) -> Vec<AgentToolBinding> {
    vec![
        AgentToolBinding::new(
            SEARCH_REVISION,
            Arc::new(SearchTool::new(agent_id, store.clone())),
            EffectRecovery::SafeToReplay,
        ),
        AgentToolBinding::new(
            LOAD_REVISION,
            Arc::new(LoadTool::new(agent_id, store.clone())),
            EffectRecovery::SafeToReplay,
        ),
        AgentToolBinding::new(
            format!("{EXECUTE_REVISION}/{CALL_BOUNDARY_REVISION}"),
            Arc::new(ExecuteTool::new(
                agent_id,
                store,
                adapter,
                authorizations,
                session_id,
                command_id,
            )),
            EffectRecovery::NeverReplay,
        ),
    ]
}

struct LoadTool {
    agent_id: AgentId,
    store: McpCatalogStore,
    spec: ToolSpec,
}

impl LoadTool {
    fn new(agent_id: AgentId, store: McpCatalogStore) -> Self {
        Self {
            agent_id,
            store,
            spec: ToolSpec {
                name: LOAD_TOOL.to_owned(),
                description: format!(
                    "Load exact descriptions and input schemas for 1-{LOAD_REFERENCE_LIMIT} references returned by tool_search. Load only tools you are about to call, then pass each unchanged reference to code_mode's Python mcp(reference, arguments), or to tool_execute when Code Mode is not selected."
                ),
                input_schema: json!({
                    "type": "object",
                    "properties": {
                        "references": {
                            "type": "array",
                            "items": {"type": "string"},
                            "minItems": 1,
                            "maxItems": LOAD_REFERENCE_LIMIT
                        }
                    },
                    "required": ["references"],
                    "additionalProperties": false
                }),
            },
        }
    }
}

impl Tool for LoadTool {
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
            let input: LoadInput = decode_call(&call, LOAD_TOOL)?;
            let references = parse_references(input.references)?;
            require_active(&cancellation)?;
            let store = self.store.clone();
            let agent_id = self.agent_id;
            let lookup = references.clone();
            let resolved = tokio::task::spawn_blocking(move || {
                store.resolve_agent_tools(&agent_id.to_string(), &lookup)
            })
            .await
            .map_err(|error| background_error(&error))?
            .map_err(host_error)?;
            require_active(&cancellation)?;
            if resolved.len() != references.len() {
                return Err(ToolError::internal(
                    "Host catalog returned the wrong number of loaded tools",
                ));
            }
            let tools = references
                .into_iter()
                .zip(resolved)
                .map(|(reference, tool)| LoadedTool {
                    reference: reference.to_string(),
                    name: tool.tool().name().to_owned(),
                    description: tool.tool().description().to_owned(),
                    input_schema: tool.tool().model_input_schema().clone(),
                })
                .collect();
            let encoded = serde_json::to_string(&LoadOutput { tools }).map_err(|error| {
                ToolError::internal(format!("tool schemas could not be encoded: {error}"))
            })?;
            if encoded.len() > LOAD_OUTPUT_BYTES {
                return Err(ToolError::output_limit(format!(
                    "loaded tool schemas exceed {LOAD_OUTPUT_BYTES} bytes; load fewer or smaller tools"
                )));
            }
            Ok(ToolOutput {
                content: vec![ContentBlock::text(encoded)],
                details: None,
                is_error: false,
            })
        })
    }
}

struct ExecuteTool {
    agent_id: AgentId,
    store: McpCatalogStore,
    adapter: Option<PathBuf>,
    authorizations: McpAuthorizationResolver,
    session_id: SessionId,
    command_id: Option<CommandId>,
    spec: ToolSpec,
}

impl ExecuteTool {
    fn new(
        agent_id: AgentId,
        store: McpCatalogStore,
        adapter: Option<PathBuf>,
        authorizations: McpAuthorizationResolver,
        session_id: SessionId,
        command_id: Option<CommandId>,
    ) -> Self {
        Self {
            agent_id,
            store,
            adapter,
            authorizations,
            session_id,
            command_id,
            spec: ToolSpec {
                name: EXECUTE_TOOL.to_owned(),
                description: "Execute one extension tool using an unchanged reference from tool_search after reading its schema with tool_load. Arguments must match that loaded schema. Stale references fail and must be searched again.".to_owned(),
                input_schema: json!({
                    "type": "object",
                    "properties": {
                        "reference": {"type": "string"},
                        "arguments": {"type": "object"}
                    },
                    "required": ["reference", "arguments"],
                    "additionalProperties": false
                }),
            },
        }
    }
}

impl Tool for ExecuteTool {
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
            let input: ExecuteInput = decode_call(&call, EXECUTE_TOOL)?;
            if !input.arguments.is_object() {
                return Err(ToolError::invalid_input(
                    "extension tool arguments must be a JSON object",
                ));
            }
            let reference = McpToolReference::from_str(&input.reference).map_err(host_error)?;
            require_active(&cancellation)?;
            let store = self.store.clone();
            let agent_id = self.agent_id;
            let stored_reference = reference.clone();
            let mut resolved = tokio::task::spawn_blocking(move || {
                store.resolve_agent_tools(&agent_id.to_string(), &[stored_reference])
            })
            .await
            .map_err(|error| background_error(&error))?
            .map_err(host_error)?;
            require_active(&cancellation)?;
            let selected = resolved.pop().ok_or_else(|| {
                ToolError::not_found("the referenced extension tool is not available")
            })?;
            let adapter = self.adapter.as_ref().ok_or_else(|| {
                ToolError::unavailable(
                    "RENOA_MCP_ADAPTER must be set before an extension tool can execute",
                )
            })?;
            let operation_id = oauth_operation_id(self.session_id, self.command_id, &call.id);
            let authorization = match self
                .authorizations
                .resolve(
                    selected.connection_id(),
                    selected.endpoint(),
                    selected.auth(),
                    &operation_id,
                    cancellation.clone(),
                )
                .await
            {
                Ok(authorization) => authorization,
                Err(error) => return authorization_failure(&reference, &selected, error),
            };
            match call_tool(
                adapter,
                &selected,
                authorization.as_ref(),
                &input.arguments,
                cancellation,
            )
            .await
            {
                Ok(result) => Ok(ToolOutput {
                    content: result.content,
                    details: Some(execution_details(
                        &reference,
                        &selected,
                        result.details.as_ref(),
                    )),
                    is_error: result.is_error,
                }),
                Err(error) => map_failure(&reference, &selected, error),
            }
        })
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct LoadInput {
    references: Vec<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ExecuteInput {
    reference: String,
    arguments: Value,
}

#[derive(Serialize)]
struct LoadOutput {
    tools: Vec<LoadedTool>,
}

#[derive(Serialize)]
struct LoadedTool {
    reference: String,
    name: String,
    description: String,
    input_schema: Value,
}

fn parse_references(encoded: Vec<String>) -> Result<Vec<McpToolReference>, ToolError> {
    if encoded.is_empty() || encoded.len() > LOAD_REFERENCE_LIMIT {
        return Err(ToolError::invalid_input(format!(
            "tool_load requires 1-{LOAD_REFERENCE_LIMIT} references"
        )));
    }
    let mut observed = HashSet::with_capacity(encoded.len());
    encoded
        .into_iter()
        .map(|encoded| {
            let reference = McpToolReference::from_str(&encoded).map_err(host_error)?;
            if !observed.insert(reference.clone()) {
                return Err(ToolError::invalid_input(format!(
                    "tool_load repeats reference `{reference}`"
                )));
            }
            Ok(reference)
        })
        .collect()
}

fn decode_call<T: DeserializeOwned>(call: &ToolCall, expected: &str) -> Result<T, ToolError> {
    if call.name != expected {
        return Err(ToolError::invalid_input(format!(
            "tool binding `{expected}` cannot execute call for `{}`",
            call.name
        )));
    }
    serde_json::from_value(call.arguments.clone()).map_err(|error| {
        ToolError::invalid_input(format!("{expected} arguments are invalid: {error}"))
    })
}

fn require_active(cancellation: &CancellationToken) -> Result<(), ToolError> {
    if cancellation.is_cancelled() {
        Err(ToolError::cancelled("tool call was cancelled", false))
    } else {
        Ok(())
    }
}

fn background_error(error: &tokio::task::JoinError) -> ToolError {
    ToolError::internal(format!("Host catalog task failed: {error}"))
}

fn host_error(error: McpHostError) -> ToolError {
    let message = error.to_string();
    match error {
        McpHostError::Invalid(_) => ToolError::invalid_input(message),
        McpHostError::Conflict(_)
        | McpHostError::OAuth(
            crate::mcp::McpOAuthError::InProgress(_)
            | crate::mcp::McpOAuthError::ReceiptUnavailable(_),
        ) => ToolError::conflict(message),
        McpHostError::NotFound(_) => ToolError::not_found(message),
        McpHostError::Io(_)
        | McpHostError::Database(_)
        | McpHostError::HostCatalog(_)
        | McpHostError::Json(_)
        | McpHostError::Background(_)
        | McpHostError::OAuth(
            crate::mcp::McpOAuthError::CallbackUnavailable(_)
            | crate::mcp::McpOAuthError::Browser { .. }
            | crate::mcp::McpOAuthError::BrowserStatus { .. },
        ) => ToolError::unavailable(message),
        McpHostError::OAuth(
            crate::mcp::McpOAuthError::AuthorizationRequired(_)
            | crate::mcp::McpOAuthError::CallbackRejected(_),
        ) => ToolError::permission_denied(message),
        McpHostError::OAuth(crate::mcp::McpOAuthError::OutcomeUnknown { .. }) => {
            ToolError::outcome_unknown(message)
        }
        McpHostError::OAuth(crate::mcp::McpOAuthError::ReceiptFailure(_)) => {
            ToolError::process_failed(message, false)
        }
        McpHostError::OAuth(crate::mcp::McpOAuthError::CallbackExpired) => {
            ToolError::timeout(message, false)
        }
        McpHostError::OAuth(crate::mcp::McpOAuthError::Cancelled) => {
            ToolError::cancelled(message, false)
        }
        McpHostError::OAuth(crate::mcp::McpOAuthError::Invalid(_)) => {
            ToolError::invalid_input(message)
        }
        McpHostError::Adapter(error) => definite_boundary_error(&error, false),
    }
}
