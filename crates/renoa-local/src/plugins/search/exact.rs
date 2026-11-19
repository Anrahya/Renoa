//! Exact lookup: the complete schemas of the plugin tools a model is about to
//! call, several in one request so a chain of calls costs one lookup.

use std::str::FromStr as _;

use renoa_agent::{ContentBlock, ToolError, ToolOutput};
use serde::Serialize;
use tokio_util::sync::CancellationToken;

use super::{PluginSearchTool, ToolMatch};
use crate::mcp::{McpToolReference, SCHEMA_LOOKUP_OUTPUT_BYTES};

const HOST_PREFIX: &str = "host:";

#[derive(Serialize)]
struct Lookup {
    tools: Vec<ToolMatch>,
    /// Set when the output schemas were left out so the input schemas,
    /// which every call needs, fit the lookup boundary.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    output_schemas_omitted: bool,
}

impl PluginSearchTool {
    /// Returns each referenced tool's complete input schema and, where its
    /// server declares one, its output schema, in the order requested.
    pub(super) async fn exact_references(
        &self,
        references: &[String],
        cancellation: CancellationToken,
    ) -> Result<ToolOutput, ToolError> {
        if references.is_empty() {
            return Err(ToolError::invalid_input(
                "references must list at least one exact tool reference from plugin_search",
            ));
        }
        let mcp = references
            .iter()
            .filter(|reference| !reference.starts_with(HOST_PREFIX))
            .map(|reference| {
                McpToolReference::from_str(reference)
                    .map_err(|error| ToolError::invalid_input(error.to_string()))
            })
            .collect::<Result<Vec<_>, _>>()?;
        let mut described = self.describe_tools(mcp, false).await?.into_iter();
        if cancellation.is_cancelled() {
            return Err(ToolError::cancelled("plugin search was cancelled", false));
        }
        let mut tools = Vec::with_capacity(references.len());
        for reference in references {
            let tool = if reference.starts_with(HOST_PREFIX) {
                let description = self
                    .host_plugins
                    .as_ref()
                    .ok_or_else(|| ToolError::not_found("Host plugins unavailable"))?
                    .exact(reference)?;
                ToolMatch {
                    reference: description.reference,
                    name: description.name,
                    description: description.description,
                    input_schema: description.input_schema,
                    output_schema: None,
                }
            } else {
                described
                    .next()
                    .ok_or_else(|| ToolError::internal("exact MCP tool was not resolved"))?
            };
            tools.push(tool);
        }
        let mut lookup = Lookup {
            tools,
            output_schemas_omitted: false,
        };
        let mut encoded = encode(&lookup)?;
        if encoded.len() > SCHEMA_LOOKUP_OUTPUT_BYTES {
            for tool in &mut lookup.tools {
                lookup.output_schemas_omitted |= tool.output_schema.take().is_some();
            }
            encoded = encode(&lookup)?;
        }
        if encoded.len() > SCHEMA_LOOKUP_OUTPUT_BYTES {
            return Err(too_large(references.len()));
        }
        Ok(ToolOutput {
            content: vec![ContentBlock::text(encoded)],
            details: None,
            is_error: false,
            closing: None,
        })
    }
}

/// The lookup failure when the input schemas of `count` tools alone pass the
/// lookup boundary.
pub(super) fn too_large(count: usize) -> ToolError {
    ToolError::output_limit(if count == 1 {
        format!(
            "this tool's input schema exceeds the {SCHEMA_LOOKUP_OUTPUT_BYTES}-byte lookup boundary"
        )
    } else {
        format!(
            "the input schemas of these {count} tools exceed the {SCHEMA_LOOKUP_OUTPUT_BYTES}-byte lookup boundary; look up fewer references at a time"
        )
    })
}

fn encode(lookup: &Lookup) -> Result<String, ToolError> {
    serde_json::to_string(lookup).map_err(|error| {
        ToolError::internal(format!("plugin tool schemas could not be encoded: {error}"))
    })
}
