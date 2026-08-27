use std::{collections::HashSet, path::PathBuf, str::FromStr as _, sync::Arc};

mod execute;

#[cfg(test)]
mod tests;

use renoa_agent::{
    BoxFuture, ContentBlock, Tool, ToolCall, ToolError, ToolOutput, ToolSpec, ToolUpdates,
};
use renoa_agent_loop::AgentToolBinding;
use renoa_kernel::EffectRecovery;
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use serde_json::{Value, json};
use tokio_util::sync::CancellationToken;

use super::{
    LOAD_OUTPUT_BYTES, LOAD_REFERENCE_LIMIT, McpAdapterError, McpCatalogStore,
    McpCredentialResolver, McpHostError, McpToolReference, SEARCH_RESULT_LIMIT,
    call::{CALL_BOUNDARY_REVISION, call_tool},
    rank_tools,
};
use crate::ALPHA_PROFILE_ID;
use execute::{definite_boundary_error, execution_details, map_failure};

const SEARCH_TOOL: &str = "tool_search";
const LOAD_TOOL: &str = "tool_load";
const EXECUTE_TOOL: &str = "tool_execute";
const REGISTRY_REVISION: &str = "renoa-mcp-registry-v1";

pub(crate) fn alpha_registry_bindings(
    store: McpCatalogStore,
    adapter: Option<PathBuf>,
    credentials: McpCredentialResolver,
) -> Vec<AgentToolBinding> {
    vec![
        AgentToolBinding::new(
            format!("{REGISTRY_REVISION}/search"),
            Arc::new(SearchTool::new(store.clone())),
            EffectRecovery::SafeToReplay,
        ),
        AgentToolBinding::new(
            format!("{REGISTRY_REVISION}/load"),
            Arc::new(LoadTool::new(store.clone())),
            EffectRecovery::SafeToReplay,
        ),
        AgentToolBinding::new(
            format!("{REGISTRY_REVISION}/execute/{CALL_BOUNDARY_REVISION}"),
            Arc::new(ExecuteTool::new(store, adapter, credentials)),
            EffectRecovery::NeverReplay,
        ),
    ]
}

struct SearchTool {
    store: McpCatalogStore,
    spec: ToolSpec,
}

impl SearchTool {
    fn new(store: McpCatalogStore) -> Self {
        Self {
            store,
            spec: ToolSpec {
                name: SEARCH_TOOL.to_owned(),
                description: format!(
                    "Find tools in Alpha's enabled extension connections without loading their schemas. Returns at most {SEARCH_RESULT_LIMIT} compact matches and exact references. Call tool_load before tool_execute. Use query `*` to browse."
                ),
                input_schema: json!({
                    "type": "object",
                    "properties": {
                        "query": {
                            "type": "string",
                            "description": "Capability, service, or tool to find; use * to browse."
                        }
                    },
                    "required": ["query"],
                    "additionalProperties": false
                }),
            },
        }
    }
}

impl Tool for SearchTool {
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
            let input: SearchInput = decode_call(&call, SEARCH_TOOL)?;
            require_active(&cancellation)?;
            let store = self.store.clone();
            let tools =
                tokio::task::spawn_blocking(move || store.alpha_tool_summaries(ALPHA_PROFILE_ID))
                    .await
                    .map_err(|error| background_error(&error))?
                    .map_err(host_error)?;
            require_active(&cancellation)?;
            let ranked = rank_tools(tools, &input.query).map_err(host_error)?;
            let matches = ranked
                .matches
                .into_iter()
                .map(|tool| {
                    Ok(SearchMatch {
                        reference: tool.reference()?.to_string(),
                        integration: tool.integration_id,
                        connection: tool.connection_id,
                        name: tool.name,
                        description: tool.description,
                    })
                })
                .collect::<Result<Vec<_>, McpHostError>>()
                .map_err(host_error)?;
            json_output(&SearchOutput {
                matches,
                total_matches: ranked.total_matches,
            })
        })
    }
}

struct LoadTool {
    store: McpCatalogStore,
    spec: ToolSpec,
}

impl LoadTool {
    fn new(store: McpCatalogStore) -> Self {
        Self {
            store,
            spec: ToolSpec {
                name: LOAD_TOOL.to_owned(),
                description: format!(
                    "Load exact descriptions and input schemas for 1-{LOAD_REFERENCE_LIMIT} references returned by tool_search. Load only tools you are about to call, then pass the unchanged reference to tool_execute."
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
            let lookup = references.clone();
            let resolved = tokio::task::spawn_blocking(move || {
                store.resolve_alpha_tools(ALPHA_PROFILE_ID, &lookup)
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
    store: McpCatalogStore,
    adapter: Option<PathBuf>,
    credentials: McpCredentialResolver,
    spec: ToolSpec,
}

impl ExecuteTool {
    fn new(
        store: McpCatalogStore,
        adapter: Option<PathBuf>,
        credentials: McpCredentialResolver,
    ) -> Self {
        Self {
            store,
            adapter,
            credentials,
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
            let stored_reference = reference.clone();
            let mut resolved = tokio::task::spawn_blocking(move || {
                store.resolve_alpha_tools(ALPHA_PROFILE_ID, &[stored_reference])
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
            let authorization = self
                .credentials
                .resolve(selected.auth(), cancellation.clone())
                .await
                .map_err(McpAdapterError::from)
                .map_err(|error| definite_boundary_error(&error, false))?;
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
struct SearchInput {
    query: String,
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
struct SearchOutput {
    matches: Vec<SearchMatch>,
    total_matches: usize,
}

#[derive(Serialize)]
struct SearchMatch {
    reference: String,
    integration: String,
    connection: String,
    name: String,
    description: String,
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

fn json_output(output: &impl Serialize) -> Result<ToolOutput, ToolError> {
    let content = serde_json::to_string(output).map_err(|error| {
        ToolError::internal(format!("tool result could not be encoded: {error}"))
    })?;
    Ok(ToolOutput {
        content: vec![ContentBlock::text(content)],
        details: None,
        is_error: false,
    })
}

fn background_error(error: &tokio::task::JoinError) -> ToolError {
    ToolError::internal(format!("Host catalog task failed: {error}"))
}

fn host_error(error: McpHostError) -> ToolError {
    let message = error.to_string();
    match error {
        McpHostError::Invalid(_) => ToolError::invalid_input(message),
        McpHostError::Conflict(_) => ToolError::conflict(message),
        McpHostError::NotFound(_) => ToolError::not_found(message),
        McpHostError::Io(_)
        | McpHostError::Database(_)
        | McpHostError::HostCatalog(_)
        | McpHostError::Json(_) => ToolError::unavailable(message),
        McpHostError::Adapter(error) => definite_boundary_error(&error, false),
    }
}
