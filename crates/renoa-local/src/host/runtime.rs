use std::sync::Arc;

use renoa_agent::AgentEventSink;
use renoa_agent_loop::AgentToolBinding;
use renoa_kernel::{CommandId, SessionId};

use super::definition::ResolvedAgentDefinition;
use super::{HostConfig, LocalHostError};
use crate::{
    LocalRuntimeConfig, LocalWorkspace, ModelChoice, ReasoningLevel,
    mcp::agent_registry_bindings,
    plugins::agent_plugin_binding,
    runtime::build_composed_local_runtime,
    skills::{agent_skill_bindings, runtime_context},
};

pub(crate) struct RuntimeRequest<'a> {
    pub(crate) definition: &'a ResolvedAgentDefinition,
    pub(crate) session_id: SessionId,
    pub(crate) command_id: Option<CommandId>,
    pub(crate) model: &'a ModelChoice,
    pub(crate) reasoning: ReasoningLevel,
    pub(crate) workspace: &'a LocalWorkspace,
    pub(crate) events: Option<Arc<dyn AgentEventSink>>,
}

/// Resolves one agent's runtime from its stored definition.
///
/// Every capability the Host can bind is offered here and the agent's exact
/// stored selection decides which bindings are kept. No policy is inferred from
/// an identity, a prefix, or a preset.
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
    } = request;
    let agent = definition.agent_id();
    let mut offered = agent_registry_bindings(
        agent,
        host.mcp_catalog.clone(),
        host.mcp_adapter.clone(),
        host.mcp_authorizations.clone(),
        session_id,
        command_id,
    );
    if let Some(binding) = definition.document_binding() {
        offered.push(binding);
    }
    offered.push(agent_plugin_binding(
        agent,
        host.plugins.clone(),
        workspace.root().to_path_buf(),
        session_id,
        command_id,
    ));
    offered.push(super::bots::tool::binding(
        Arc::clone(host),
        session_id,
        command_id,
    ));
    offered.push(super::routines::result_tool::binding(
        Arc::clone(host),
        session_id,
    ));
    offered.push(super::routines::tool::binding(
        Arc::clone(host),
        session_id,
        command_id,
    ));
    offered.extend(agent_skill_bindings(
        agent,
        host.skill_store.clone(),
        workspace.root().to_path_buf(),
        session_id,
        command_id,
    ));
    let selection = &definition.selected_tools().tools;
    let extension_tools: Vec<AgentToolBinding> = offered
        .into_iter()
        .filter(|binding| selection.contains(binding.tool_name()))
        .collect();
    let skills = host.skill_store.clone();
    let skill_context =
        tokio::task::spawn_blocking(move || runtime_context(&skills, session_id, command_id))
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
    .with_reasoning(reasoning);
    if let Some(skill_context) = skill_context {
        config = config.with_skill_context(skill_context);
    }
    Ok(build_composed_local_runtime(config, workspace, extension_tools, events).await?)
}
