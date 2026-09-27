use std::sync::Arc;

use renoa_agent::{BoxFuture, Tool, ToolCall, ToolError, ToolOutput, ToolSpec, ToolUpdates};
use renoa_agent_loop::AgentToolBinding;
use renoa_kernel::EffectRecovery;
use serde::Deserialize;
use serde_json::{Value, json};
use tokio_util::sync::CancellationToken;

use super::HostPlugins;

pub(crate) fn binding(
    plugins: Arc<HostPlugins>,
    mcp: &AgentToolBinding,
) -> Result<AgentToolBinding, ToolError> {
    let revision = format!(
        "renoa-plugin-executor-v1/{}/{}",
        plugins.revision()?,
        mcp.revision()
    );
    let spec = ToolSpec {
        name: crate::capabilities::TOOL_EXECUTE.to_owned(),
        description: "Invoke one enabled plugin tool. Copy the exact reference and full input_schema from plugin_search, and pass a matching arguments object. If input_schema was absent, first request plugin_search with reference alone. Stale references require a new search.".to_owned(),
        input_schema: json!({
            "type": "object",
            "properties": {
                "reference": {"type": "string"},
                "arguments": {"type": "object"}
            },
            "required": ["reference", "arguments"],
            "additionalProperties": false
        }),
    };
    Ok(AgentToolBinding::new(
        revision,
        Arc::new(Execute {
            plugins,
            mcp: mcp.tool(),
            spec,
        }),
        EffectRecovery::NeverReplay,
    ))
}
struct Execute {
    plugins: Arc<HostPlugins>,
    mcp: Arc<dyn Tool>,
    spec: ToolSpec,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Input {
    reference: String,
    arguments: Value,
}
impl Tool for Execute {
    fn spec(&self) -> &ToolSpec {
        &self.spec
    }
    fn execute(
        &self,
        call: ToolCall,
        cancellation: CancellationToken,
        updates: ToolUpdates,
    ) -> BoxFuture<'_, Result<ToolOutput, ToolError>> {
        Box::pin(async move {
            if call.name != self.spec.name {
                return Err(ToolError::invalid_input("wrong plugin executor binding"));
            }
            let input: Input = serde_json::from_value(call.arguments.clone())
                .map_err(|error| ToolError::invalid_input(error.to_string()))?;
            if !input.arguments.is_object() {
                return Err(ToolError::invalid_input(
                    "plugin arguments must be an object",
                ));
            }
            if cancellation.is_cancelled() {
                return Err(ToolError::cancelled(
                    "plugin call cancelled before execution",
                    false,
                ));
            }
            if input.reference.starts_with("host:") {
                let tool = self.plugins.resolve(&input.reference)?;
                tool.execute(
                    ToolCall {
                        name: tool.spec().name.clone(),
                        arguments: input.arguments,
                        ..call
                    },
                    cancellation,
                    updates,
                )
                .await
            } else {
                self.mcp.execute(call, cancellation, updates).await
            }
        })
    }
}
