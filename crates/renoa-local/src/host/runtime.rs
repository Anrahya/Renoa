use std::{collections::HashSet, sync::Arc};

use renoa_agent::AgentEventSink;
use renoa_agent_loop::{AgentToolBinding, CodeModeBinding, code_mode_spec};
use renoa_kernel::{AgentId, CommandId, EffectAdapter, SessionId};

use super::definition::{ResolvedAgentDefinition, agent_manage_binding};
use super::{HostConfig, LocalHostError};
use crate::{
    LocalRuntimeConfig, LocalWorkspace, ModelChoice, ReasoningLevel,
    mcp::agent_registry_bindings,
    plugins::agent_plugin_binding,
    runtime::build_composed_local_runtime,
    skills::{agent_skill_bindings, runtime_context},
    speaker::ToolAccess,
};

pub(crate) struct RuntimeRequest<'a> {
    pub(crate) definition: &'a ResolvedAgentDefinition,
    pub(crate) session_id: SessionId,
    pub(crate) command_id: Option<CommandId>,
    pub(crate) model: &'a ModelChoice,
    pub(crate) reasoning: ReasoningLevel,
    pub(crate) workspace: &'a LocalWorkspace,
    pub(crate) events: Option<Arc<dyn AgentEventSink>>,
    /// Whether the turn's tool calls run; see [`ToolAccess`].
    pub(crate) tools: ToolAccess,
}

/// Resolves one agent's runtime from its stored definition.
///
/// Machine tools follow the exact stored grants. Every agent receives the plugin
/// protocol; enabled Host and external plugins are discovered through it, and
/// the resident tools of its Host plugins are bound beside it.
pub(crate) async fn resolve_runtime(
    host: &Arc<HostConfig>,
    request: RuntimeRequest<'_>,
) -> Result<renoa_kernel::Runtime, LocalHostError> {
    let RuntimeRequest {
        definition,
        session_id,
        command_id,
        model,
        reasoning,
        workspace,
        events,
        tools,
    } = request;
    let offered = tools.apply(offered_tool_bindings(
        host, definition, workspace, session_id, command_id, tools,
    )?);
    let (extension_tools, code_mode) = bind_code_mode(host, offered)?;
    let skills = host.skill_store.clone();
    let skill_context =
        tokio::task::spawn_blocking(move || runtime_context(&skills, session_id, command_id, ""))
            .await??;
    let mut config = LocalRuntimeConfig::for_definition(
        host.bridge.clone(),
        model.provider().as_str(),
        model.id(),
        host.credential_store.clone(),
        definition,
        workspace,
    )?
    .with_discovered_model(model)
    .with_session(session_id)
    .with_reasoning(reasoning)
    .with_tool_access(tools);
    if let Some(code_mode) = code_mode {
        config = config.with_code_mode(code_mode);
    }
    if let Some(skill_context) = skill_context {
        config = config.with_skill_context(skill_context);
    }
    Ok(build_composed_local_runtime(config, workspace, extension_tools, events).await?)
}

pub(crate) fn synthetic_activity_tools_for_definition(
    host: &Arc<HostConfig>,
    definition: &ResolvedAgentDefinition,
    workspace: &LocalWorkspace,
    session_id: SessionId,
) -> Result<HashSet<String>, LocalHostError> {
    let offered = offered_tool_bindings(
        host,
        definition,
        workspace,
        session_id,
        None,
        ToolAccess::Refused,
    )?;
    let (extension_tools, _) = bind_code_mode(host, offered)?;
    let direct_tools =
        workspace.selected_kernel_tool_bindings(Some(&definition.selected_tools().tools));
    let mut names = HashSet::new();
    for binding in direct_tools.into_iter().chain(extension_tools) {
        let tool = binding.tool();
        let spec = tool.spec();
        if renoa_agent::schema_accepts_synthetic_activity(&spec.input_schema) {
            names.insert(spec.name.clone());
        }
    }
    if host.code_mode.is_some()
        && renoa_agent::schema_accepts_synthetic_activity(&code_mode_spec().input_schema)
    {
        names.insert(code_mode_spec().name);
    }
    Ok(names)
}

pub(crate) fn bind_code_mode(
    host: &HostConfig,
    offered: Vec<AgentToolBinding>,
) -> Result<(Vec<AgentToolBinding>, Option<CodeModeBinding>), LocalHostError> {
    let code_selected = host.code_mode.is_some();
    let (extension_tools, hidden_executor) = partition_selected_tools(offered, code_selected)?;
    let code_mode = if code_selected {
        let evaluator = host.code_mode.as_ref().ok_or_else(|| {
            LocalHostError::Configuration(
                "code_mode is selected, but no exact-pinned Monty worker is configured".to_owned(),
            )
        })?;
        let execute = hidden_executor.ok_or_else(|| {
            LocalHostError::Configuration("Plugin executor binding is unavailable".to_owned())
        })?;
        let adapter: Arc<dyn EffectAdapter> = Arc::clone(evaluator) as Arc<dyn EffectAdapter>;
        Some(CodeModeBinding::new(
            crate::code_mode::MontyEvaluator::revision(),
            adapter,
            execute,
        ))
    } else {
        None
    };
    Ok((extension_tools, code_mode))
}

fn partition_selected_tools(
    offered: Vec<AgentToolBinding>,
    code_selected: bool,
) -> Result<(Vec<AgentToolBinding>, Option<AgentToolBinding>), LocalHostError> {
    let mut visible = Vec::new();
    let mut hidden_executor = None;
    for binding in offered {
        if code_selected && binding.tool_name() == crate::capabilities::TOOL_EXECUTE {
            if hidden_executor.replace(binding).is_some() {
                return Err(LocalHostError::Configuration(
                    "Plugin executor binding is configured twice".to_owned(),
                ));
            }
        } else {
            visible.push(binding);
        }
    }
    Ok((visible, hidden_executor))
}

fn offered_tool_bindings(
    host: &Arc<HostConfig>,
    definition: &ResolvedAgentDefinition,
    workspace: &LocalWorkspace,
    session_id: SessionId,
    command_id: Option<CommandId>,
    access: ToolAccess,
) -> Result<Vec<AgentToolBinding>, LocalHostError> {
    protocol_bindings(
        host,
        definition,
        workspace.root(),
        session_id,
        command_id,
        access,
        workspace
            .kernel_tool_bindings()
            .into_iter()
            .filter(|binding| binding.tool_name().starts_with("git_"))
            .collect(),
    )
}

/// The tools for messaging other agents, offered only while a node has
/// attached the way to reach them.
fn peer_bindings(
    host: &HostConfig,
    agent: AgentId,
    command_id: Option<CommandId>,
) -> Vec<AgentToolBinding> {
    let peers = host
        .peers
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .clone();
    peers.map_or_else(Vec::new, |peers| {
        super::peers::bindings(peers, agent, command_id).into()
    })
}

pub(crate) fn protocol_bindings(
    host: &Arc<HostConfig>,
    definition: &ResolvedAgentDefinition,
    workspace: &std::path::Path,
    session_id: SessionId,
    command_id: Option<CommandId>,
    access: ToolAccess,
    git: Vec<AgentToolBinding>,
) -> Result<Vec<AgentToolBinding>, LocalHostError> {
    let agent = definition.agent_id();
    let mut controls = Vec::new();
    if let Some(binding) = definition.document_binding() {
        controls.push(binding);
    }
    controls.push(agent_manage_binding(
        Arc::clone(host),
        agent,
        session_id,
        command_id,
    ));
    controls.extend(super::automations::bindings(
        host, agent, session_id, command_id,
    ));
    controls.extend(peer_bindings(host, agent, command_id));
    controls.extend(crate::plugins::host::owner::bindings(
        crate::plugins::host::owner::OwnerTurn {
            database: host.database.clone(),
            agent,
            session: session_id,
            command: command_id,
            workspace: workspace.to_path_buf(),
            principal: definition.principal(),
        },
    ));
    controls.extend(crate::plugins::host::memory::bindings(
        crate::plugins::host::memory::MemoryTurn {
            database: host.database.clone(),
            data_directory: host.home.path().to_path_buf(),
            author: crate::plugins::host::memory::author(definition.name(), agent),
            principal: definition.principal(),
        },
    ));
    controls.extend(agent_skill_bindings(
        agent,
        host.skill_store.clone(),
        workspace.to_path_buf(),
        session_id,
        command_id,
    ));
    controls.extend(git);
    let plugins = Arc::new(
        crate::plugins::host::HostPlugins::new(host.database.clone(), agent, controls)
            .map_err(|error| LocalHostError::Configuration(error.to_string()))?,
    );
    let executor = agent_registry_bindings(
        agent,
        host.mcp_catalog.clone(),
        host.mcp_adapter.clone(),
        host.mcp_authorizations.clone(),
        session_id,
        command_id,
    )
    .pop()
    .ok_or_else(|| LocalHostError::Configuration("MCP executor is unavailable".to_owned()))?;
    // A turn with no person has no notebook to list tools for. A guest's turn
    // shares a person's conversation, so it lists them like every tool it
    // refuses, and the history's earlier calls keep their definitions.
    let notebook = definition.principal().is_some() || access == ToolAccess::Refused;
    let resident = crate::plugins::host::resident::bindings(&plugins, notebook);
    let mut bindings = vec![
        crate::plugins::search_binding_with_host(agent, host.plugins.clone(), Arc::clone(&plugins)),
        agent_plugin_binding(
            agent,
            host.plugins.clone(),
            workspace.to_path_buf(),
            session_id,
            command_id,
        ),
        crate::plugins::host::executor::binding(plugins, &executor)
            .map_err(|error| LocalHostError::Configuration(error.to_string()))?,
    ];
    bindings.extend(resident);
    Ok(bindings)
}

#[cfg(test)]
mod execution_tests;
#[cfg(test)]
mod memory_dream_tests;
#[cfg(test)]
mod memory_pass_tests;
#[cfg(test)]
mod memory_plugin_tests;
#[cfg(test)]
mod memory_reminder_tests;
#[cfg(test)]
mod owner_plugin_tests;
#[cfg(test)]
mod time_plugin_tests;

#[cfg(test)]
mod tests;
