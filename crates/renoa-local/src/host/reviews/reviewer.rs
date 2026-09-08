use std::{
    fmt::Write as _,
    num::{NonZeroU32, NonZeroU64},
    sync::Arc,
};

use renoa_agent_loop::{AgentLoopConfig, AgentToolBinding, ModelBinding, build_runtime};
use renoa_kernel::{EffectRecovery, Runtime, SessionId};
use sha2::{Digest as _, Sha256};

use super::{GitHubReviewError, GitHubReviewSnapshot, github::GitHub};
use crate::{BridgeModel, LocalHostError, host::HostConfig};

pub(super) const SOURCE_CALLS_PER_RESPONSE: u32 = 50;

pub(super) const INSTRUCTIONS: &str = include_str!("review_instructions.txt");

pub(super) async fn runtime(
    host: &HostConfig,
    snapshot: &GitHubReviewSnapshot,
    tools: &ReviewTools<'_>,
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
    let config = AgentLoopConfig::until_complete(
        &snapshot.system_prompt,
        NonZeroU32::new(SOURCE_CALLS_PER_RESPONSE).expect("nonzero tool budget"),
    );
    let context = crate::runtime::context_binding(
        &model,
        None,
        Some(crate::profile::AutomaticCompactionPolicy {
            trigger_input_tokens: NonZeroU64::new(258_400).expect("nonzero compaction trigger"),
            target_input_tokens: NonZeroU64::new(155_040).expect("nonzero compaction target"),
        }),
        NonZeroU64::new(272_000),
    )?;
    Ok(build_runtime(
        config,
        context,
        ModelBinding::new(revision, model, EffectRecovery::SafeToReplay),
        tools.bindings(),
    )
    .map_err(GitHubReviewError::from)?)
}

pub(super) struct ReviewTools<'a> {
    pub(super) github: &'a GitHub,
    pub(super) source: ReviewToolSource<'a>,
}
pub(super) enum ReviewToolSource<'a> {
    Sandbox(&'a Arc<crate::isolated_workspace::InspectionSandbox>),
    #[cfg(test)]
    Fixture(&'a GitHubReviewSnapshot),
}
impl ReviewTools<'_> {
    fn bindings(&self) -> Vec<AgentToolBinding> {
        match self.source {
            ReviewToolSource::Sandbox(container) => container.bindings(),
            #[cfg(test)]
            ReviewToolSource::Fixture(snapshot) => {
                super::tests::source_tool::bindings(self.github.clone(), snapshot)
            }
        }
    }
}
