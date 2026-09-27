use super::{PREVIEW_SCHEMA_BYTES, PluginSearchTool, ToolMatch};
use crate::{
    mcp::{McpHostError, McpToolReference},
    plugins::{PluginError, tool::output::plugin_error},
};
use renoa_agent::ToolError;

impl PluginSearchTool {
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
