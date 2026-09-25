use std::{str::FromStr as _, sync::Arc};

use renoa_agent::{
    BoxFuture, ContentBlock, Tool, ToolCall, ToolError, ToolOutput, ToolSpec, ToolUpdates,
};
use renoa_agent_loop::AgentToolBinding;
use renoa_kernel::{AgentId, EffectRecovery};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tokio_util::sync::CancellationToken;

use super::{
    PluginError, PluginManager,
    tool::output::{json_output, plugin_error, registry_error_output},
};
use crate::{
    mcp::{
        McpConnectionStatus, McpHostError, McpToolReference, SCHEMA_LOOKUP_OUTPUT_BYTES,
        SEARCH_RESULT_LIMIT, rank_tools,
    },
    output::MAX_TOOL_OUTPUT_BYTES,
};

mod local;
#[cfg(test)]
mod tests;

const TOOL_NAME: &str = crate::capabilities::PLUGIN_SEARCH;
const BINDING_REVISION: &str = "renoa-plugin-search-v2";
const PREVIEW_LIMIT: usize = 3;
const PREVIEW_SCHEMA_BYTES: usize = 4 * 1024;

pub(crate) fn binding(
    agent_id: AgentId,
    manager: PluginManager,
    can_manage: bool,
) -> AgentToolBinding {
    AgentToolBinding::new(
        BINDING_REVISION,
        Arc::new(PluginSearchTool::new(agent_id, manager, can_manage)),
        EffectRecovery::SafeToReplay,
    )
}

pub(crate) struct PluginSearchTool {
    agent_id: AgentId,
    manager: PluginManager,
    can_manage: bool,
    spec: ToolSpec,
}

impl PluginSearchTool {
    pub(crate) fn new(agent_id: AgentId, manager: PluginManager, can_manage: bool) -> Self {
        Self {
            agent_id,
            manager,
            can_manage,
            spec: ToolSpec {
                name: TOOL_NAME.to_owned(),
                description: format!(
                    "Search the Host plugin library. Start with a targeted query; use query=* only to browse. A targeted local search returns plugin cards and up to {PREVIEW_LIMIT} matching MCP tools with exact references. If a match includes input_schema, use that complete schema to call code_mode's Python mcp(reference, arguments), or tool_execute when Code Mode is absent. If input_schema is absent, call plugin_search with only reference to get the full schema before calling the tool. Pass plugin to inspect components and connections; pass an enabled connection to list up to {SEARCH_RESULT_LIMIT} MCP tools per page. source=official_mcp_registry researches external candidates and never installs them. A loaded catalog or configured credential does not guarantee a remote call will succeed."
                ),
                input_schema: json!({
                    "type": "object",
                    "properties": {
                        "query": {"type":"string", "minLength":1, "maxLength":256, "description":"Search text for local plugins, nested connection tools, or the official Registry. Use * only to browse locally. Omit for exact plugin, reference, or Registry name/version lookup."},
                        "plugin": {"type":"string", "description":"Exact plugin id returned by local search. Inspect this plugin's components and visible connection status."},
                        "connection": {"type":"string", "description":"Exact enabled connection id returned by plugin inspection. Return nested MCP tools and exact references."},
                        "reference": {"type":"string", "description":"Exact MCP tool reference returned by local search. Use alone to get its complete model-facing input_schema; never combine with query, plugin, connection, offset, or Registry fields."},
                        "offset": {"type":"integer", "minimum":0, "description":"Next offset returned by a local page; omit for the first page."},
                        "source": {"type":"string", "enum":["local", "official_mcp_registry"], "description":"Defaults to local. Use official_mcp_registry only to research an external MCP candidate."},
                        "registry_name": {"type":"string", "description":"Exact publisher/server name returned by official Registry search; pair with registry_version to inspect the external record."},
                        "registry_version": {"type":"string", "description":"Exact version returned by official Registry search; latest is rejected."}
                    },
                    "additionalProperties": false
                }),
            },
        }
    }

    async fn local_inventory(&self) -> Result<local::Inventory, ToolError> {
        let (packages, shared_refresh_unavailable) = match self.manager.list_report().await {
            Ok(packages) => (packages, false),
            Err(_) => (
                self.manager
                    .local_list_report()
                    .await
                    .map_err(|error| plugin_error(error, false))?,
                true,
            ),
        };
        let connections = self
            .manager
            .connection_statuses(&self.agent_id)
            .await
            .map_err(|error| plugin_error(error, false))?;
        let connections = if self.can_manage {
            connections
        } else {
            connections
                .into_iter()
                .filter(McpConnectionStatus::enabled_for_agent)
                .collect()
        };
        let skills = self
            .manager
            .skill_source_reports(&self.agent_id)
            .await
            .map_err(|error| plugin_error(error, false))?;
        let tools = self
            .manager
            .tool_summaries(&self.agent_id)
            .await
            .map_err(|error| plugin_error(error, false))?;
        Ok(local::Inventory::new(
            &packages,
            &connections,
            &skills,
            tools,
            shared_refresh_unavailable,
        ))
    }

    async fn run(
        &self,
        input: SearchInput,
        cancellation: CancellationToken,
    ) -> Result<ToolOutput, ToolError> {
        if cancellation.is_cancelled() {
            return Err(ToolError::cancelled("plugin search was cancelled", false));
        }
        match input.source.as_deref().unwrap_or("local") {
            "local" => self.run_local(input, cancellation).await,
            "official_mcp_registry" => self.run_registry(input, cancellation).await,
            _ => Err(ToolError::invalid_input(
                "source must be local or official_mcp_registry",
            )),
        }
    }

    async fn run_local(
        &self,
        input: SearchInput,
        cancellation: CancellationToken,
    ) -> Result<ToolOutput, ToolError> {
        if input.registry_name.is_some() || input.registry_version.is_some() {
            return Err(ToolError::invalid_input(
                "registry_name and registry_version require source=official_mcp_registry",
            ));
        }
        if let Some(reference) = input.reference.as_deref() {
            if input.query.is_some()
                || input.plugin.is_some()
                || input.connection.is_some()
                || input.offset != 0
                || input.source.is_some()
            {
                return Err(ToolError::invalid_input(
                    "reference must be the only plugin_search argument",
                ));
            }
            return self.exact_reference(reference, cancellation).await;
        }
        let inventory = self.local_inventory().await?;
        if cancellation.is_cancelled() {
            return Err(ToolError::cancelled("plugin search was cancelled", false));
        }
        match (input.plugin, input.connection) {
            (Some(plugin), None) if input.query.is_none() => {
                json_output(&inventory.inspect(&plugin, input.offset)?)
            }
            (None, Some(connection)) => {
                let query = input.query.as_deref().unwrap_or("*");
                self.search_connection(&inventory, &connection, query, input.offset)
                    .await
            }
            (None, None) => {
                let query = input.query.as_deref().ok_or_else(|| {
                    ToolError::invalid_input("local plugin search requires query")
                })?;
                self.search_cards(&inventory, query, input.offset).await
            }
            _ => Err(ToolError::invalid_input(
                "inspect a plugin without query or connection, or search tools by connection without plugin",
            )),
        }
    }

    async fn exact_reference(
        &self,
        encoded: &str,
        cancellation: CancellationToken,
    ) -> Result<ToolOutput, ToolError> {
        let reference = McpToolReference::from_str(encoded)
            .map_err(|error| ToolError::invalid_input(error.to_string()))?;
        let mut tools = self.describe_tools(vec![reference], false).await?;
        if cancellation.is_cancelled() {
            return Err(ToolError::cancelled("plugin search was cancelled", false));
        }
        let tool = tools
            .pop()
            .ok_or_else(|| ToolError::internal("exact MCP tool was not resolved"))?;
        let encoded = serde_json::to_string(&tool).map_err(|error| {
            ToolError::internal(format!("MCP tool schema could not be encoded: {error}"))
        })?;
        if encoded.len() > SCHEMA_LOOKUP_OUTPUT_BYTES {
            return Err(ToolError::output_limit(format!(
                "exact MCP tool schema exceeds the {SCHEMA_LOOKUP_OUTPUT_BYTES}-byte output boundary"
            )));
        }
        Ok(ToolOutput {
            content: vec![ContentBlock::text(encoded)],
            details: None,
            is_error: false,
        })
    }

    async fn search_connection(
        &self,
        inventory: &local::Inventory,
        connection: &str,
        query: &str,
        offset: usize,
    ) -> Result<ToolOutput, ToolError> {
        let ranked = rank_tools(inventory.tools(connection)?, query, offset)
            .map_err(|error| ToolError::invalid_input(error.to_string()))?;
        let mut matches = ranked
            .matches
            .into_iter()
            .map(|tool| {
                Ok(ToolMatch {
                    reference: tool
                        .reference()
                        .map_err(|error| ToolError::internal(error.to_string()))?
                        .to_string(),
                    name: tool.name().to_owned(),
                    description: tool.description().to_owned(),
                    input_schema: None,
                })
            })
            .collect::<Result<Vec<_>, ToolError>>()?;
        if query.trim() != "*" {
            let references = matches
                .iter()
                .take(PREVIEW_LIMIT)
                .map(|tool| {
                    McpToolReference::from_str(&tool.reference)
                        .map_err(|error| ToolError::internal(error.to_string()))
                })
                .collect::<Result<Vec<_>, _>>()?;
            let previews = self.describe_tools(references, true).await?;
            for preview in previews {
                if let Some(item) = matches
                    .iter_mut()
                    .find(|item| item.reference == preview.reference)
                {
                    item.input_schema = preview.input_schema;
                }
            }
        }
        json_output(&local::Page::new(
            matches,
            ranked.total_matches,
            offset,
            inventory.shared_refresh_unavailable(),
        )?)
    }

    async fn search_cards(
        &self,
        inventory: &local::Inventory,
        query: &str,
        offset: usize,
    ) -> Result<ToolOutput, ToolError> {
        let mut page = inventory.search(query, offset)?;
        if query.trim() == "*" || offset != 0 {
            return json_output(&page);
        }
        let ranked = rank_tools(inventory.all_tools(), query, 0)
            .map_err(|error| ToolError::invalid_input(error.to_string()))?;
        let references = ranked
            .matches
            .into_iter()
            .take(PREVIEW_LIMIT)
            .map(|tool| {
                tool.reference()
                    .map_err(|error| ToolError::internal(error.to_string()))
            })
            .collect::<Result<Vec<_>, _>>()?;
        let mut matches = self.describe_tools(references, true).await?;
        loop {
            let result = SearchResult {
                page: &page,
                tool_matches: &matches,
            };
            if serde_json::to_vec(&result)
                .map_err(|error| ToolError::internal(error.to_string()))?
                .len()
                <= MAX_TOOL_OUTPUT_BYTES
            {
                return json_output(&result);
            }
            if page.shorten(offset) {
                continue;
            }
            if matches.pop().is_none() {
                return json_output(&page);
            }
        }
    }

    async fn run_registry(
        &self,
        input: SearchInput,
        cancellation: CancellationToken,
    ) -> Result<ToolOutput, ToolError> {
        if input.plugin.is_some()
            || input.connection.is_some()
            || input.reference.is_some()
            || input.offset != 0
        {
            return Err(ToolError::invalid_input(
                "official Registry search does not accept plugin, connection, reference, or offset",
            ));
        }
        match (input.query, input.registry_name, input.registry_version) {
            (Some(query), None, None) => {
                match self.manager.search_registry(&query, cancellation).await {
                    Ok(result) => json_output(&RegistryOutput {
                        action: "search",
                        installed: false,
                        result,
                    }),
                    Err(error) => registry_error_output(error),
                }
            }
            (None, Some(name), Some(version)) => match self
                .manager
                .lookup_registry(&name, &version, cancellation)
                .await
            {
                Ok(result) => json_output(&RegistryOutput {
                    action: "lookup",
                    installed: false,
                    result,
                }),
                Err(error) => registry_error_output(error),
            },
            _ => Err(ToolError::invalid_input(
                "provide query for official Registry search, or exact registry_name and registry_version for lookup",
            )),
        }
    }

    async fn describe_tools(
        &self,
        references: Vec<McpToolReference>,
        preview: bool,
    ) -> Result<Vec<ToolMatch>, ToolError> {
        if references.is_empty() {
            return Ok(Vec::new());
        }
        let store = self.manager.mcp_catalog();
        let agent_id = self.agent_id;
        let expected = references.len();
        let resolved = tokio::task::spawn_blocking(move || {
            let mut resolved = Vec::with_capacity(references.len());
            for reference in references {
                match store
                    .resolve_agent_tools(&agent_id.to_string(), std::slice::from_ref(&reference))
                {
                    Ok(mut tools) if tools.len() == 1 => {
                        resolved.push((reference, tools.remove(0)));
                    }
                    Ok(_) => {
                        return Err(McpHostError::Invalid(
                            "Host catalog returned the wrong number of MCP tools".to_owned(),
                        ));
                    }
                    Err(McpHostError::Conflict(_) | McpHostError::NotFound(_)) if preview => {}
                    Err(error) => return Err(error),
                }
            }
            Ok(resolved)
        })
        .await
        .map_err(|error| ToolError::internal(format!("Host catalog task failed: {error}")))?
        .map_err(|error| plugin_error(PluginError::Mcp(error), false))?;
        if !preview && resolved.len() != expected {
            return Err(ToolError::internal(
                "Host catalog returned the wrong number of MCP tools",
            ));
        }
        resolved
            .into_iter()
            .map(|(reference, resolved)| {
                let schema = resolved.tool().model_input_schema();
                let include_schema = !preview
                    || serde_json::to_vec(schema)
                        .map_err(|error| {
                            ToolError::internal(format!(
                                "MCP tool schema could not be encoded: {error}"
                            ))
                        })?
                        .len()
                        <= PREVIEW_SCHEMA_BYTES;
                Ok(ToolMatch {
                    reference: reference.to_string(),
                    name: resolved.tool().name().to_owned(),
                    description: if preview {
                        resolved.tool().description().chars().take(320).collect()
                    } else {
                        resolved.tool().description().to_owned()
                    },
                    input_schema: include_schema.then(|| schema.clone()),
                })
            })
            .collect()
    }
}

impl Tool for PluginSearchTool {
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
            if call.name != TOOL_NAME {
                return Err(ToolError::invalid_input(format!(
                    "tool binding `{TOOL_NAME}` cannot execute call for `{}`",
                    call.name
                )));
            }
            let input = serde_json::from_value(call.arguments).map_err(|error| {
                ToolError::invalid_input(format!("{TOOL_NAME} arguments are invalid: {error}"))
            })?;
            self.run(input, cancellation).await
        })
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SearchInput {
    query: Option<String>,
    plugin: Option<String>,
    connection: Option<String>,
    reference: Option<String>,
    #[serde(default)]
    offset: usize,
    source: Option<String>,
    registry_name: Option<String>,
    registry_version: Option<String>,
}

#[derive(Clone, Serialize)]
struct ToolMatch {
    reference: String,
    name: String,
    description: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    input_schema: Option<Value>,
}

#[derive(Serialize)]
struct SearchResult<'a> {
    #[serde(flatten)]
    page: &'a local::Page<local::PluginCard>,
    #[serde(skip_serializing_if = "<[ToolMatch]>::is_empty")]
    tool_matches: &'a [ToolMatch],
}

#[derive(Serialize)]
struct RegistryOutput<T> {
    action: &'static str,
    installed: bool,
    #[serde(flatten)]
    result: T,
}
