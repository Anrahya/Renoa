use std::str::FromStr as _;

use super::{PREVIEW_LIMIT, PREVIEW_SCHEMA_BYTES, PluginSearchTool, ToolMatch, local, page};
use crate::{
    mcp::{McpHostError, McpToolReference, McpToolSummary, rank_tools},
    plugins::{
        PluginError,
        tool::output::{json_output, plugin_error},
    },
};
use renoa_agent::{ToolError, ToolOutput};

impl PluginSearchTool {
    pub(super) async fn search_tools(
        &self,
        inventory: &local::Inventory,
        tools: Vec<McpToolSummary>,
        query: &str,
        offset: usize,
    ) -> Result<ToolOutput, ToolError> {
        let ranked = rank_tools(tools, query, offset)
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
        json_output(&page::Page::new(
            matches,
            ranked.total_matches,
            offset,
            inventory.shared_refresh_unavailable(),
        )?)
    }

    pub(super) async fn describe_tools(
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
