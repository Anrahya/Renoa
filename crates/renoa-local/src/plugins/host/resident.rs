//! Resident tools: the tools a compiled plugin keeps in the agent's own tool
//! list, so a model calls them without searching first.
//!
//! [`HostPluginId::resident_tools`] names them. [`HostPlugins`] keeps the one
//! registration of each tool, and a resident binding wraps that same instance,
//! so `plugin_search`, `tool_execute` and the direct call run one
//! implementation under one revision.
//!
//! The kernel freezes a command's tool list into its manifest, and a retried or
//! recovered command must rebuild the same list, so the list never depends on
//! whether a plugin is enabled: every resident tool is listed, and each call
//! checks its plugin's state, as `tool_execute` checks a reference's, and is
//! refused while the plugin is off.

use std::sync::Arc;

use renoa_agent::{BoxFuture, Tool, ToolCall, ToolError, ToolOutput, ToolSpec, ToolUpdates};
use renoa_agent_loop::AgentToolBinding;
use tokio_util::sync::CancellationToken;

use super::{HostPluginId, HostPlugins};

const BINDING_REVISION: &str = "renoa-host-resident-v1";

/// The resident tools of every compiled plugin, in plugin and declaration
/// order; `notebook` as for [`HostPluginId::resident_tools`]. A declared tool
/// this agent has no registration for, such as `agent_documents` for an agent
/// that keeps no documents, is left out.
pub(crate) fn bindings(plugins: &Arc<HostPlugins>, notebook: bool) -> Vec<AgentToolBinding> {
    let mut bindings = Vec::new();
    for plugin in HostPluginId::ALL {
        let Some(registered) = plugins.plugins.get(&plugin) else {
            continue;
        };
        let names = plugin.resident_tools(notebook);
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
    bindings
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
            let (plugins, plugin) = (Arc::clone(&self.plugins), self.plugin);
            // Plugin state is a catalog read, which blocks.
            let enabled = tokio::task::spawn_blocking(move || plugins.enabled(plugin))
                .await
                .map_err(|error| {
                    ToolError::internal(format!("the plugin state check failed: {error}"))
                })??;
            if !enabled {
                return Err(ToolError::not_found(format!(
                    "{} is turned off for this agent, so {} cannot run. Answer without it; if the person wants it back, turn it on with plugin_manage enable_plugin.",
                    plugin.id(),
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
            for notebook in [true, false] {
                for name in plugin.resident_tools(notebook) {
                    assert_eq!(HostPluginId::owner(name), Some(plugin), "{name}");
                }
            }
        }
    }
}
