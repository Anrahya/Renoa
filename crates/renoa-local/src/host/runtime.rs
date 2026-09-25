use std::{collections::BTreeSet, sync::Arc};

use renoa_agent::AgentEventSink;
use renoa_agent_loop::{AgentToolBinding, CodeModeBinding};
use renoa_kernel::{CommandId, EffectAdapter, SessionId};

use super::definition::{ResolvedAgentDefinition, agent_manage_binding};
use super::{HostConfig, LocalHostError};
use crate::{
    LocalRuntimeConfig, LocalWorkspace, ModelChoice, ReasoningLevel,
    mcp::agent_registry_bindings,
    plugins::{agent_plugin_binding, agent_plugin_search_binding},
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
    let offered = offered_tool_bindings(host, definition, workspace, session_id, command_id);
    let selection = &definition.selected_tools().tools;
    let code_selected = selection.contains(crate::capabilities::CODE_MODE);
    let (extension_tools, hidden_executor) = partition_selected_tools(offered, selection)?;
    let code_mode = if code_selected {
        let evaluator = host.code_mode.as_ref().ok_or_else(|| {
            LocalHostError::Configuration(
                "code_mode is selected, but no exact-pinned Monty worker is configured".to_owned(),
            )
        })?;
        if host.mcp_adapter.is_none() {
            return Err(LocalHostError::Configuration(
                "code_mode requires an MCP adapter".to_owned(),
            ));
        }
        let execute = hidden_executor.ok_or_else(|| {
            LocalHostError::Configuration("MCP executor binding is unavailable".to_owned())
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
    if let Some(code_mode) = code_mode {
        config = config.with_code_mode(code_mode);
    }
    if let Some(skill_context) = skill_context {
        config = config.with_skill_context(skill_context);
    }
    Ok(build_composed_local_runtime(config, workspace, extension_tools, events).await?)
}

fn partition_selected_tools(
    offered: Vec<AgentToolBinding>,
    selection: &BTreeSet<String>,
) -> Result<(Vec<AgentToolBinding>, Option<AgentToolBinding>), LocalHostError> {
    let code_selected = selection.contains(crate::capabilities::CODE_MODE);
    let mut visible = Vec::new();
    let mut hidden_executor = None;
    for binding in offered {
        if code_selected && binding.tool_name() == crate::capabilities::TOOL_EXECUTE {
            if hidden_executor.replace(binding).is_some() {
                return Err(LocalHostError::Configuration(
                    "MCP executor binding is configured twice".to_owned(),
                ));
            }
        } else if selection.contains(binding.tool_name()) {
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
) -> Vec<AgentToolBinding> {
    let agent = definition.agent_id();
    let can_manage_plugins = definition
        .selected_tools()
        .tools
        .contains(crate::capabilities::PLUGIN_MANAGE);
    let mut offered = vec![agent_plugin_search_binding(
        agent,
        host.plugins.clone(),
        can_manage_plugins,
    )];
    offered.extend(agent_registry_bindings(
        agent,
        host.mcp_catalog.clone(),
        host.mcp_adapter.clone(),
        host.mcp_authorizations.clone(),
        session_id,
        command_id,
    ));
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
    offered.push(agent_manage_binding(
        Arc::clone(host),
        agent,
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
    offered
}

#[cfg(test)]
mod tests {
    use std::{collections::BTreeSet, fs};

    use renoa_agent_loop::AgentToolBinding;
    use tokio_util::sync::CancellationToken;
    use uuid::Uuid;

    use super::{offered_tool_bindings, partition_selected_tools};
    use crate::{
        AgentCreateRequest, AgentCreationOrigin, AgentCreator, AgentPresetId, LocalWorkspace,
        ModelProvider,
        host::{HostInitialization, LocalHost},
        presets::ARCEE_PRESET_ID,
    };

    #[tokio::test]
    async fn every_non_workspace_catalog_component_has_one_runtime_binding() {
        let directory = tempfile::tempdir().expect("fixture");
        let root = directory.path();
        fs::create_dir(root.join("workspace")).expect("workspace");
        fs::write(root.join("model.mjs"), "// fixture\n").expect("model");
        fs::write(root.join("auth.sqlite"), "").expect("auth boundary");
        let host = LocalHost::assemble(HostInitialization {
            data_directory: root.join("data"),
            bridge: root.join("model.mjs"),
            providers: vec![ModelProvider::OpenCodeGo],
            initial_provider: ModelProvider::OpenCodeGo,
            initial_model: "fixture".to_owned(),
            initial_reasoning: None,
            credential_store: root.join("auth.sqlite"),
            mcp_adapter: None,
            mcp_registry_adapter: None,
            shared_plugin_registry: None,
            global_skill_source: None,
            oauth_relay: None,
            code_mode: None,
        })
        .expect("Host");
        let definition = host
            .create_agent(
                AgentCreator::System {
                    component: "catalog-test".to_owned(),
                },
                AgentCreationOrigin::Provisioning,
                AgentCreateRequest::new(
                    Uuid::new_v4(),
                    AgentPresetId::new(ARCEE_PRESET_ID).expect("preset id"),
                    "Catalog",
                ),
                CancellationToken::new(),
            )
            .await
            .expect("agent");
        let resolved = host
            .resolve_definition(definition.id)
            .await
            .expect("definition");
        let workspace = LocalWorkspace::open(root.join("workspace")).expect("open workspace");
        let offered = offered_tool_bindings(
            &host.config,
            &resolved,
            &workspace,
            renoa_kernel::SessionId::from_uuid(Uuid::new_v4()),
            None,
        );
        let actual: BTreeSet<&str> = offered.iter().map(AgentToolBinding::tool_name).collect();
        assert_eq!(
            actual.len(),
            offered.len(),
            "runtime tool names must be unique"
        );
        let expected: BTreeSet<&str> = [
            "plugin_manage",
            "agent_manage",
            "routine_manage",
            "routine_results",
            "plugin_search",
            "tool_execute",
            "skill_search",
            "skill_load",
            "agent_documents",
        ]
        .into_iter()
        .collect();
        assert_eq!(actual, expected);
        let selection = BTreeSet::from([
            "code_mode".to_owned(),
            "tool_execute".to_owned(),
            "plugin_search".to_owned(),
        ]);
        let (visible, hidden) =
            partition_selected_tools(offered, &selection).expect("partition selected tools");
        assert_eq!(visible.len(), 1);
        assert_eq!(visible[0].tool_name(), "plugin_search");
        assert_eq!(
            hidden.expect("hidden MCP executor").tool_name(),
            "tool_execute"
        );
    }
}
