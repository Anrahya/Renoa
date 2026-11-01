//! Compiled Host capabilities use the same discovery and invocation boundary as MCP plugins.
//! Imported manifests cannot register implementations or grant machine access.
//! A compiled plugin contributes tools, per-message context, session context,
//! or a mix of them, and may keep some of its tools resident in the agent's own
//! tool list; see [`resident`].

use std::{collections::BTreeMap, path::PathBuf, sync::Arc};

use renoa_agent::{Tool, ToolError, ToolSpec};
use renoa_agent_loop::AgentToolBinding;
use renoa_kernel::{AgentId, EffectRecovery};
use serde::Serialize;

pub(crate) mod executor;
pub(crate) mod memory;
pub(crate) mod message_context;
pub(crate) mod resident;
pub(crate) mod session_context;
pub(crate) mod settings;
pub(crate) mod state;
pub(crate) mod time;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum HostPluginId {
    Agents,
    Automations,
    Documents,
    Skills,
    Git,
    Time,
    Memory,
}

impl HostPluginId {
    pub(crate) const ALL: [Self; 7] = [
        Self::Agents,
        Self::Automations,
        Self::Documents,
        Self::Skills,
        Self::Git,
        Self::Time,
        Self::Memory,
    ];
    pub(crate) const fn id(self) -> &'static str {
        match self {
            Self::Agents => "renoa.agents",
            Self::Automations => "renoa.automations",
            Self::Documents => "renoa.documents",
            Self::Skills => "renoa.skills",
            Self::Git => "renoa.git",
            Self::Time => time::PLUGIN_ID,
            Self::Memory => memory::PLUGIN_ID,
        }
    }
    pub(crate) fn parse(id: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|plugin| plugin.id() == id)
    }
    pub(crate) const fn description(self) -> &'static str {
        match self {
            Self::Agents => {
                "Create, list, and rename Host agents, and message the owner's other agents."
            }
            Self::Automations => "Schedule agents and read automation results.",
            Self::Documents => {
                "Edit this agent's SOUL document and the USER document of the person it talks to."
            }
            Self::Skills => "Find skills and pin their instructions to this session.",
            Self::Git => {
                "Inspect local Git changes, diffs, and pinned commits without shell access."
            }
            Self::Time => time::DESCRIPTION,
            Self::Memory => memory::DESCRIPTION,
        }
    }
    /// The tools this plugin keeps in the agent's own tool list, besides
    /// finding them through `plugin_search`. A model searches only when a
    /// request plainly needs a tool, so a tool it should reach for unasked,
    /// such as noting what a person tells it, is resident. Each name is one
    /// this plugin owns; see [`Self::owner`].
    ///
    /// `notebook` is whether the turn lists a person's notebook tools: it
    /// talks to a person, or it is a guest's turn in their conversation.
    pub(crate) const fn resident_tools(self, notebook: bool) -> &'static [&'static str] {
        match self {
            Self::Documents => &[crate::capabilities::AGENT_DOCUMENTS],
            Self::Memory if notebook => &[memory::NOTE, memory::RECALL, memory::ZOOM],
            Self::Memory
            | Self::Agents
            | Self::Automations
            | Self::Skills
            | Self::Git
            | Self::Time => &[],
        }
    }
    pub(crate) fn owner(tool: &str) -> Option<Self> {
        match tool {
            "agent_manage" | "agent_roster" | "message_agent" => Some(Self::Agents),
            "automation_manage" | "automation_results" => Some(Self::Automations),
            "agent_documents" => Some(Self::Documents),
            "skill_search" | "skill_load" => Some(Self::Skills),
            "git_changes" | "git_diff" | "git_show" => Some(Self::Git),
            memory::NOTE | memory::RECALL | memory::ZOOM => Some(Self::Memory),
            _ => None,
        }
    }
}

/// A contributor one point left out of one command, and why: `surface`, a
/// plugin id, or `*` when no plugin could be asked.
#[derive(Debug, serde::Serialize)]
pub(crate) struct Skipped {
    pub(crate) source: &'static str,
    pub(crate) reason: String,
}

struct RegisteredTool {
    tool: Arc<dyn Tool>,
    revision: String,
    /// The binding's own class, which a resident binding keeps; `tool_execute`
    /// never replays.
    recovery: EffectRecovery,
}

pub(crate) struct HostPlugins {
    database: PathBuf,
    agent: AgentId,
    plugins: BTreeMap<HostPluginId, BTreeMap<String, RegisteredTool>>,
}

#[derive(Clone, Serialize)]
pub(crate) struct HostToolDescription {
    pub(crate) reference: String,
    pub(crate) name: String,
    pub(crate) description: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) input_schema: Option<serde_json::Value>,
}

impl HostPlugins {
    pub(crate) fn revision(&self) -> Result<String, ToolError> {
        let revisions: BTreeMap<_, BTreeMap<_, _>> = self
            .plugins
            .iter()
            .map(|(plugin, tools)| {
                (
                    plugin.id(),
                    tools
                        .iter()
                        .map(|(name, tool)| (name, &tool.revision))
                        .collect(),
                )
            })
            .collect();
        Ok(crate::mcp::hex_sha256(
            &serde_json::to_vec(&revisions)
                .map_err(|error| ToolError::internal(error.to_string()))?,
        ))
    }
    pub(crate) fn new(
        database: PathBuf,
        agent: AgentId,
        bindings: Vec<AgentToolBinding>,
    ) -> Result<Self, ToolError> {
        let mut plugins = BTreeMap::<HostPluginId, BTreeMap<String, RegisteredTool>>::new();
        for binding in bindings {
            let name = binding.tool_name().to_owned();
            let owner = HostPluginId::owner(&name)
                .ok_or_else(|| ToolError::internal("tool has no Host plugin owner"))?;
            let spec = binding.tool().spec().clone();
            let revision = crate::mcp::hex_sha256(
                &serde_json::to_vec(&(binding.revision(), &spec))
                    .map_err(|error| ToolError::internal(error.to_string()))?,
            );
            if plugins
                .entry(owner)
                .or_default()
                .insert(
                    name,
                    RegisteredTool {
                        tool: binding.tool(),
                        revision,
                        recovery: binding.recovery(),
                    },
                )
                .is_some()
            {
                return Err(ToolError::internal("duplicate Host plugin tool"));
            }
        }
        Ok(Self {
            database,
            agent,
            plugins,
        })
    }
    pub(crate) fn enabled(&self, plugin: HostPluginId) -> Result<bool, ToolError> {
        state::enabled(&self.database, self.agent, plugin)
            .map_err(|error| ToolError::internal(error.to_string()))
    }
    pub(crate) fn components(
        &self,
    ) -> impl Iterator<Item = (HostPluginId, Vec<HostToolDescription>)> {
        HostPluginId::ALL
            .into_iter()
            .map(|plugin| (plugin, self.describe(plugin, false)))
    }
    fn describe(&self, plugin: HostPluginId, schema: bool) -> Vec<HostToolDescription> {
        self.plugins
            .get(&plugin)
            .into_iter()
            .flat_map(|tools| tools.values())
            .map(|registered| {
                let spec = registered.tool.spec();
                HostToolDescription {
                    reference: format!(
                        "host:{}:{}:{}",
                        plugin.id(),
                        registered.revision,
                        spec.name
                    ),
                    name: spec.name.clone(),
                    description: spec.description.clone(),
                    input_schema: schema.then(|| spec.input_schema.clone()),
                }
            })
            .collect()
    }
    pub(crate) fn matches(
        &self,
        plugin: Option<&str>,
        query: &str,
        schema: bool,
    ) -> Result<Vec<HostToolDescription>, ToolError> {
        let tokens: Vec<String> = query
            .to_lowercase()
            .split(|c: char| !c.is_alphanumeric())
            .filter(|token| !token.is_empty())
            .map(str::to_owned)
            .collect();
        if query != "*" && tokens.is_empty() {
            return Err(ToolError::invalid_input(
                "query must contain a letter or digit",
            ));
        }
        let mut found = Vec::new();
        for owner in HostPluginId::ALL {
            if plugin.is_some_and(|id| id != owner.id()) || !self.enabled(owner)? {
                continue;
            }
            for description in self.describe(owner, schema) {
                let text = format!(
                    "{} {} {}",
                    owner.id(),
                    description.name,
                    description.description
                )
                .to_lowercase();
                if query == "*" || tokens.iter().all(|token| text.contains(token)) {
                    found.push(description);
                }
            }
        }
        Ok(found)
    }
    pub(crate) fn resolve(&self, encoded: &str) -> Result<Arc<dyn Tool>, ToolError> {
        let parts: Vec<&str> = encoded.split(':').collect();
        let ["host", plugin, revision, name] = parts.as_slice() else {
            return Err(ToolError::invalid_input(
                "invalid Host plugin tool reference",
            ));
        };
        let owner = HostPluginId::parse(plugin)
            .ok_or_else(|| ToolError::not_found("unknown Host plugin"))?;
        if !self.enabled(owner)? {
            return Err(ToolError::not_found(
                "plugin is disabled; enable it with plugin_manage",
            ));
        }
        let selected = self
            .plugins
            .get(&owner)
            .and_then(|tools| tools.get(*name))
            .ok_or_else(|| {
                ToolError::not_found("Host plugin tool is unavailable for this agent")
            })?;
        if selected.revision != *revision {
            return Err(ToolError::invalid_input(
                "stale Host tool reference; search again",
            ));
        }
        Ok(Arc::clone(&selected.tool))
    }
    pub(crate) fn exact(&self, encoded: &str) -> Result<HostToolDescription, ToolError> {
        let tool = self.resolve(encoded)?;
        let ToolSpec {
            name,
            description,
            input_schema,
        } = tool.spec().clone();
        Ok(HostToolDescription {
            reference: encoded.to_owned(),
            name,
            description,
            input_schema: Some(input_schema),
        })
    }
}
