use std::sync::Arc;

use renoa_agent::AgentEventSink;
use renoa_kernel::{CommandId, SessionId};

use super::{HostConfig, LocalHostError};
use crate::{
    AgentProfile, LocalRuntimeConfig, LocalWorkspace, ModelChoice, ReasoningLevel,
    mcp::profile_registry_bindings,
    plugins::profile_plugin_binding,
    runtime::build_composed_local_runtime,
    skills::{profile_skill_bindings, runtime_context},
};

pub(crate) struct RuntimeRequest<'a> {
    pub(crate) profile: &'a AgentProfile,
    pub(crate) session_id: SessionId,
    pub(crate) command_id: Option<CommandId>,
    pub(crate) model: &'a ModelChoice,
    pub(crate) reasoning: ReasoningLevel,
    pub(crate) workspace: &'a LocalWorkspace,
    pub(crate) events: Option<Arc<dyn AgentEventSink>>,
}

pub(crate) async fn resolve_runtime(
    host: &HostConfig,
    request: RuntimeRequest<'_>,
) -> Result<renoa_kernel::Runtime, LocalHostError> {
    let RuntimeRequest {
        profile,
        session_id,
        command_id,
        model,
        reasoning,
        workspace,
        events,
    } = request;
    let mut extension_tools = profile_registry_bindings(
        profile.id().clone(),
        host.mcp_catalog.clone(),
        host.mcp_adapter.clone(),
        host.mcp_credentials.clone(),
        session_id,
        command_id,
    );
    extension_tools.push(profile_plugin_binding(
        profile.id().clone(),
        host.plugins.clone(),
        workspace.root().to_path_buf(),
        session_id,
        command_id,
    ));
    extension_tools.extend(profile_skill_bindings(
        profile.id().clone(),
        host.skill_store.clone(),
        workspace.root().to_path_buf(),
        session_id,
        command_id,
    ));
    let skills = host.skill_store.clone();
    let skill_context =
        tokio::task::spawn_blocking(move || runtime_context(&skills, session_id, command_id))
            .await??;
    let mut config = LocalRuntimeConfig::for_profile(
        host.bridge.clone(),
        model.provider().as_str(),
        model.id(),
        host.credential_store.clone(),
        profile,
        workspace,
    )?
    .with_discovered_model(model)
    .with_reasoning(reasoning);
    if let Some(skill_context) = skill_context {
        config = config.with_skill_context(skill_context);
    }
    Ok(build_composed_local_runtime(config, workspace, extension_tools, events).await?)
}
