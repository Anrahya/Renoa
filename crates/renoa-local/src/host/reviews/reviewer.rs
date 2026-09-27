use std::{
    fmt::Write as _,
    num::{NonZeroU32, NonZeroU64},
    sync::Arc,
};

use renoa_agent_loop::{
    AgentLoopConfig, AgentToolBinding, ModelBinding, build_runtime_with_code_mode_and_events,
    build_runtime_with_events,
};
use renoa_kernel::{CommandId, EffectRecovery, Runtime, SessionId};
use sha2::{Digest as _, Sha256};

use super::{GitHubReviewError, GitHubReviewSnapshot, github::GitHub};
use crate::{BridgeModel, LocalHostError, host::HostConfig};

pub(super) const SOURCE_CALLS_PER_RESPONSE: u32 = 50;

pub(super) const INSTRUCTIONS: &str = include_str!("review_instructions.txt");

pub(super) async fn runtime(
    host: &Arc<HostConfig>,
    snapshot: &GitHubReviewSnapshot,
    tools: &ReviewTools<'_>,
    command: CommandId,
    events: Arc<dyn renoa_agent::AgentEventSink>,
) -> Result<Runtime, LocalHostError> {
    let model = Arc::new(
        BridgeModel::load_with_spec(
            host.bridge.clone(),
            snapshot.provider.as_str(),
            &snapshot.model,
            host.credential_store.clone(),
            Some(snapshot.model_spec.clone()),
            Some(snapshot.reasoning),
            NonZeroU32::new(32_768).expect("nonzero output allowance"),
        )
        .await?
        .with_session(Some(SessionId::from_uuid(snapshot.request.id))),
    );
    let mut expected_binding = String::with_capacity(64);
    for byte in Sha256::digest(snapshot.model_spec.as_bytes()) {
        write!(&mut expected_binding, "{byte:02x}").expect("writing to a String cannot fail");
    }
    if model.binding_id() != expected_binding || model.reasoning() != snapshot.reasoning {
        return Err(LocalHostError::Configuration(
            "review model no longer matches its frozen specification/reasoning".to_owned(),
        ));
    }
    let revision = format!(
        "renoa.review.model/v1/{}/{}/{}/{}",
        snapshot.provider,
        snapshot.model,
        model.binding_id(),
        snapshot.reasoning.as_str()
    );
    let skills = host.skill_store.clone();
    let frozen_prompt = snapshot.system_prompt.clone();
    let session = SessionId::from_uuid(snapshot.request.id);
    let skill_context = tokio::task::spawn_blocking(move || {
        crate::skills::runtime_context(&skills, session, Some(command), &frozen_prompt)
    })
    .await??;
    let mut instructions = snapshot.system_prompt.clone();
    if let Some(context) = &skill_context
        && !context.instructions.is_empty()
    {
        instructions.push_str("\n\n");
        instructions.push_str(&context.instructions);
    }
    let config = AgentLoopConfig::until_complete(
        &instructions,
        NonZeroU32::new(SOURCE_CALLS_PER_RESPONSE).expect("nonzero tool budget"),
    );
    let context = crate::runtime::context_binding(
        &model,
        skill_context.as_ref(),
        Some(crate::AutomaticCompaction {
            trigger_input_tokens: NonZeroU64::new(258_400).expect("nonzero compaction trigger"),
            target_input_tokens: NonZeroU64::new(155_040).expect("nonzero compaction target"),
        }),
        NonZeroU64::new(272_000),
    )?;
    let mut bound_tools = tools.bindings(snapshot.tools.as_ref());
    if let Some(workspace) = tools.workspace() {
        let definition = crate::host::definition::resolve_definition(
            host,
            snapshot.request.repository.policy.agent_id,
        )
        .await?;
        let git = tools
            .bindings(None)
            .into_iter()
            .filter(|binding| binding.tool_name().starts_with("git_"))
            .collect();
        let protocol = crate::host::runtime::protocol_bindings(
            host,
            &definition,
            workspace,
            SessionId::from_uuid(snapshot.request.id),
            Some(command),
            git,
        )?;
        bound_tools.extend(protocol);
    }
    let model = ModelBinding::new(revision, model, EffectRecovery::SafeToReplay);
    let (bound_tools, code_mode) = if tools.workspace().is_some() {
        crate::host::runtime::bind_code_mode(host, bound_tools)?
    } else {
        (bound_tools, None)
    };
    let runtime = if let Some(code_mode) = code_mode {
        build_runtime_with_code_mode_and_events(
            config,
            context,
            model,
            bound_tools,
            code_mode,
            events,
        )
    } else {
        build_runtime_with_events(config, context, model, bound_tools, events)
    };
    Ok(runtime.map_err(GitHubReviewError::from)?)
}

pub(super) struct ReviewTools<'a> {
    pub(super) github: &'a GitHub,
    pub(super) source: ReviewToolSource<'a>,
}
pub(super) enum ReviewToolSource<'a> {
    Sandbox(&'a Arc<crate::isolated_workspace::InspectionSandbox>),
    #[cfg(test)]
    Workspace(&'a crate::LocalWorkspace),
    #[cfg(test)]
    Fixture(&'a GitHubReviewSnapshot),
}
impl ReviewTools<'_> {
    #[cfg_attr(
        not(test),
        expect(
            clippy::unnecessary_wraps,
            reason = "the fixture review source deliberately has no workspace or Host plugin bindings"
        )
    )]
    fn workspace(&self) -> Option<&std::path::Path> {
        match self.source {
            ReviewToolSource::Sandbox(container) => Some(container.checkout()),
            #[cfg(test)]
            ReviewToolSource::Workspace(workspace) => Some(workspace.root()),
            #[cfg(test)]
            ReviewToolSource::Fixture(_) => None,
        }
    }

    fn bindings(
        &self,
        selected: Option<&std::collections::BTreeSet<String>>,
    ) -> Vec<AgentToolBinding> {
        match self.source {
            ReviewToolSource::Sandbox(container) => container.bindings(selected),
            #[cfg(test)]
            ReviewToolSource::Workspace(workspace) => {
                workspace.selected_kernel_tool_bindings(selected)
            }
            #[cfg(test)]
            ReviewToolSource::Fixture(snapshot) => {
                super::tests::source_tool::bindings(self.github.clone(), snapshot)
            }
        }
    }
}
