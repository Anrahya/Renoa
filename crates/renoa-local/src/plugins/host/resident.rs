//! Resident tools: the tools a compiled plugin keeps in the agent's own tool
//! list while it is enabled, so a model calls them without searching first.
//!
//! [`HostPluginId::resident_tools`] names them. [`HostPlugins`] keeps the one
//! registration of each tool, and a resident binding wraps that same instance,
//! so `plugin_search`, `tool_execute` and the direct call run one
//! implementation under one revision. Enablement is read when a runtime
//! resolves, which decides whether the tool is listed, and again on every call,
//! as `tool_execute` does, so a plugin turned off during a command refuses its
//! resident tools from then on.

use std::sync::Arc;

use renoa_agent::{BoxFuture, Tool, ToolCall, ToolError, ToolOutput, ToolSpec, ToolUpdates};
use renoa_agent_loop::AgentToolBinding;
use tokio_util::sync::CancellationToken;

use super::{HostPluginId, HostPlugins};

const BINDING_REVISION: &str = "renoa-host-resident-v1";

/// The resident tools of every plugin enabled for the agent, in plugin and
/// declaration order. A declared tool this agent has no registration for,
/// such as `agent_documents` for an agent that keeps no documents, is left out.
pub(crate) fn bindings(plugins: &Arc<HostPlugins>) -> Result<Vec<AgentToolBinding>, ToolError> {
    let mut bindings = Vec::new();
    for plugin in HostPluginId::ALL {
        let names = plugin.resident_tools();
        if names.is_empty() || !plugins.enabled(plugin)? {
            continue;
        }
        let Some(registered) = plugins.plugins.get(&plugin) else {
            continue;
        };
        for registered in names.iter().filter_map(|name| registered.get(*name)) {
            bindings.push(AgentToolBinding::new(
                format!("{BINDING_REVISION}/{}/{}", plugin.id(), registered.revision),
                Arc::new(Resident {
                    plugins: Arc::clone(plugins),
                    plugin,
                    tool: Arc::clone(&registered.tool),
                }),
                registered.recovery,
            ));
        }
    }
    Ok(bindings)
}

struct Resident {
    plugins: Arc<HostPlugins>,
    plugin: HostPluginId,
    tool: Arc<dyn Tool>,
}

impl Tool for Resident {
    fn spec(&self) -> &ToolSpec {
        self.tool.spec()
    }

    fn execute(
        &self,
        call: ToolCall,
        cancellation: CancellationToken,
        updates: ToolUpdates,
    ) -> BoxFuture<'_, Result<ToolOutput, ToolError>> {
        Box::pin(async move {
            if !self.plugins.enabled(self.plugin)? {
                return Err(ToolError::not_found(format!(
                    "{} is turned off for this agent, so {} cannot run; turn it on with plugin_manage enable_plugin first",
                    self.plugin.id(),
                    self.tool.spec().name
                )));
            }
            self.tool.execute(call, cancellation, updates).await
        })
    }
}

#[cfg(test)]
mod tests {
    use super::HostPluginId;

    /// A resident name is one its plugin registers; a name another plugin owns,
    /// or none does, would never be bound.
    #[test]
    fn every_resident_tool_is_owned_by_the_plugin_that_declares_it() {
        for plugin in HostPluginId::ALL {
            for name in plugin.resident_tools() {
                assert_eq!(HostPluginId::owner(name), Some(plugin), "{name}");
            }
        }
    }
}
