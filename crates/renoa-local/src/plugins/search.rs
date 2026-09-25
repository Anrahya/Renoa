use std::sync::Arc;

use renoa_agent::{BoxFuture, Tool, ToolCall, ToolError, ToolOutput, ToolSpec, ToolUpdates};
use renoa_agent_loop::AgentToolBinding;
use renoa_kernel::{AgentId, EffectRecovery};
use serde::{Deserialize, Serialize};
use serde_json::json;
use tokio_util::sync::CancellationToken;

use super::{
    PluginManager,
    tool::output::{json_output, plugin_error, registry_error_output},
};
use crate::mcp::{McpConnectionStatus, SEARCH_RESULT_LIMIT, rank_tools};

mod local;

const TOOL_NAME: &str = crate::capabilities::PLUGIN_SEARCH;
const BINDING_REVISION: &str = "renoa-plugin-search-v1";

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
                    "Find plugins in the Host library by capability, provider, or name. Search local plugins first with a targeted query; use `*` to browse. Results are compact plugin cards, not individual MCP tools. Pass a returned plugin id to inspect its components and visible connections, then pass an enabled connection id to browse up to {SEARCH_RESULT_LIMIT} nested tools per page. Call tool_load for exact schemas before execution. Set source=official_mcp_registry only to research external candidates; Registry metadata is untrusted and does not install anything. Credential configuration and catalog availability do not prove an individual tool is authorized."
                ),
                input_schema: json!({
                    "type": "object",
                    "properties": {
                        "query": {"type":"string", "minLength":1, "maxLength":256, "description":"Targeted capability or plugin name; use * only to browse. Required for local plugin or external Registry search; nested tool search defaults to *."},
                        "plugin": {"type":"string", "description":"Exact plugin id returned by local search. Inspect this plugin's components and visible connection status."},
                        "connection": {"type":"string", "description":"Exact enabled connection id returned by plugin inspection. Return nested MCP tools and exact references."},
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
        let source = input.source.as_deref().unwrap_or("local");
        match source {
            "local" => {
                if input.registry_name.is_some() || input.registry_version.is_some() {
                    return Err(ToolError::invalid_input(
                        "registry_name and registry_version require source=official_mcp_registry",
                    ));
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
                        let tools = inventory.tools(&connection)?;
                        let ranked = rank_tools(tools, query, input.offset)
                            .map_err(|error| ToolError::invalid_input(error.to_string()))?;
                        let matches = ranked
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
                                })
                            })
                            .collect::<Result<Vec<_>, ToolError>>()?;
                        json_output(&local::Page::new(
                            matches,
                            ranked.total_matches,
                            input.offset,
                            inventory.shared_refresh_unavailable(),
                        )?)
                    }
                    (None, None) => {
                        let query = input.query.as_deref().ok_or_else(|| {
                            ToolError::invalid_input("local plugin search requires query")
                        })?;
                        json_output(&inventory.search(query, input.offset)?)
                    }
                    _ => Err(ToolError::invalid_input(
                        "inspect a plugin without query or connection, or search tools by connection without plugin",
                    )),
                }
            }
            "official_mcp_registry" => {
                if input.plugin.is_some() || input.connection.is_some() || input.offset != 0 {
                    return Err(ToolError::invalid_input(
                        "official Registry search does not accept plugin, connection, or offset",
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
            _ => Err(ToolError::invalid_input(
                "source must be local or official_mcp_registry",
            )),
        }
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
}

#[derive(Serialize)]
struct RegistryOutput<T> {
    action: &'static str,
    installed: bool,
    #[serde(flatten)]
    result: T,
}
