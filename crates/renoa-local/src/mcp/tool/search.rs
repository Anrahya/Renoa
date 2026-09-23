use renoa_agent::{
    BoxFuture, ContentBlock, Tool, ToolCall, ToolError, ToolOutput, ToolSpec, ToolUpdates,
};
use renoa_kernel::AgentId;
use serde::{Deserialize, Serialize};
use serde_json::json;
use tokio_util::sync::CancellationToken;

use super::{SEARCH_TOOL, background_error, decode_call, host_error, require_active};
use crate::mcp::{McpCatalogStore, McpHostError, SEARCH_RESULT_LIMIT, rank_tools};

const SEARCH_OUTPUT_BYTES: usize = 16 * 1_024;

pub(super) struct SearchTool {
    agent_id: AgentId,
    store: McpCatalogStore,
    spec: ToolSpec,
}

impl SearchTool {
    pub(super) fn new(agent_id: AgentId, store: McpCatalogStore) -> Self {
        Self {
            agent_id,
            store,
            spec: ToolSpec {
                name: SEARCH_TOOL.to_owned(),
                description: format!(
                    "Find enabled tools without loading schemas. Returns a page of at most {SEARCH_RESULT_LIMIT} compact matches and exact references. Use query `*` to browse; pass next_offset with the same query for another page. Call tool_load before executing a reference."
                ),
                input_schema: json!({
                    "type": "object",
                    "properties": {
                        "query": {
                            "type": "string",
                            "description": "Capability, service, or tool to find; use * to browse."
                        },
                        "offset": {
                            "type": "integer",
                            "minimum": 0,
                            "description": "Start at this match index; omit for the first page."
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
            let agent_id = self.agent_id;
            let tools = tokio::task::spawn_blocking(move || {
                store.agent_tool_summaries(&agent_id.to_string())
            })
            .await
            .map_err(|error| background_error(&error))?
            .map_err(host_error)?;
            require_active(&cancellation)?;
            let ranked = rank_tools(tools, &input.query, input.offset).map_err(host_error)?;
            let matches = ranked
                .matches
                .into_iter()
                .map(|tool| {
                    Ok(SearchMatch {
                        reference: tool.reference()?.to_string(),
                        name: tool.name,
                        description: tool.description,
                    })
                })
                .collect::<Result<Vec<_>, McpHostError>>()
                .map_err(host_error)?;
            let mut output = SearchOutput {
                matches,
                total_matches: ranked.total_matches,
                next_offset: None,
            };
            loop {
                let consumed = input.offset.saturating_add(output.matches.len());
                output.next_offset = (consumed < output.total_matches).then_some(consumed);
                let encoded = serde_json::to_string(&output).map_err(|error| {
                    ToolError::internal(format!("tool search result could not be encoded: {error}"))
                })?;
                if encoded.len() <= SEARCH_OUTPUT_BYTES {
                    return Ok(ToolOutput {
                        content: vec![ContentBlock::text(encoded)],
                        details: None,
                        is_error: false,
                    });
                }
                if output.matches.pop().is_none() {
                    return Err(ToolError::output_limit(
                        "one tool search match exceeds the output budget",
                    ));
                }
            }
        })
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SearchInput {
    query: String,
    #[serde(default)]
    offset: usize,
}

#[derive(Serialize)]
struct SearchOutput {
    matches: Vec<SearchMatch>,
    total_matches: usize,
    next_offset: Option<usize>,
}

#[derive(Serialize)]
struct SearchMatch {
    reference: String,
    name: String,
    description: String,
}
