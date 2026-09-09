use renoa_kernel::SessionId;
use tokio_util::sync::CancellationToken;

use super::{
    GitHubReviewRequest, GitHubReviewRun, GitHubReviewSnapshot, context,
    github::{GitHub, Pull},
    reviewer,
};
use crate::{
    LocalHost, LocalHostError, TurnObservation,
    host::{discover_profile_models, initial_reasoning, require_model},
};

impl LocalHost {
    pub(super) async fn prepare_review_source(
        &self,
        request: GitHubReviewRequest,
        pull: &Pull,
        github: &GitHub,
        root: Option<&std::path::Path>,
        cancellation: &CancellationToken,
    ) -> Result<Box<GitHubReviewSnapshot>, LocalHostError> {
        let mut context = context::gather(github, pull, cancellation).await?;
        if let Some(root) = root {
            super::checkout::materialize(
                root,
                &pull.base.sha,
                &pull.head.sha,
                &context.merge_base_sha,
                github,
                cancellation,
            )
            .await?;
            context
                .load_inventory(root, &pull.head.sha, cancellation)
                .await?;
        } else {
            #[cfg(test)]
            context
                .fixture_inventory(github, pull, cancellation)
                .await?;
        }
        self.prepare_review(
            request,
            pull.base.sha.clone(),
            pull.head.sha.clone(),
            context,
        )
        .await
    }

    pub(super) async fn prepare_review(
        &self,
        request: GitHubReviewRequest,
        base_sha: String,
        head_sha: String,
        context: context::ReviewContext,
    ) -> Result<Box<GitHubReviewSnapshot>, LocalHostError> {
        let agent = self
            .agent(request.repository.policy.agent_id)
            .await?
            .ok_or(LocalHostError::AgentNotFound(
                request.repository.policy.agent_id,
            ))?;
        let profile = self.profile(&agent.profile).await?;
        let models = discover_profile_models(&self.config, &profile).await?;
        let provider = profile
            .model_provider()
            .unwrap_or(self.config.initial_provider);
        let model = require_model(&models, provider, &self.config.initial_model, "review")?;
        let reasoning = initial_reasoning(model, self.config.initial_reasoning)?;
        let recipe = self
            .bot(agent.id)
            .await?
            .ok_or(LocalHostError::AgentNotFound(agent.id))?;
        let skills = self.config.skill_store.clone();
        let workspace = self.config.database.with_file_name("review-sessions");
        let skill_profile = agent.profile.as_str().to_owned();
        let id = request.id;
        let skill = tokio::task::spawn_blocking(move || {
            crate::skills::frozen_instructions(
                &skills,
                &skill_profile,
                &workspace,
                SessionId::from_uuid(id),
                renoa_kernel::CommandId::from_uuid(id),
                "renoa-code-review",
            )
        })
        .await??;
        let tools = if context.source == context::ReviewSource::GitCommits {
            Some(self.bot_tool_selection(agent.id).await?.tools)
        } else {
            None
        };
        let snapshot = Box::new(GitHubReviewSnapshot {
            request,
            base_sha,
            head_sha,
            provider,
            model: model.id().to_owned(),
            reasoning,
            prepared_at_ms: TurnObservation::now()?.unix_milliseconds(),
            model_spec: model.encoded_spec(),
            tools,
            system_prompt: format!(
                "{}\n\nBatch at most {} tool calls in one response. Retrieve source and diffs through the available tools; every page has continuation information.\n\nHost-owned reviewer identity: {}\nHost-owned reviewer instructions:\n{}\n\n{}",
                reviewer::INSTRUCTIONS,
                reviewer::SOURCE_CALLS_PER_RESPONSE,
                recipe.recipe.name,
                recipe.recipe.instructions,
                skill
            ),
            context,
        });
        self.save_review(GitHubReviewRun::Prepared {
            snapshot: snapshot.clone(),
        })
        .await?;
        Ok(snapshot)
    }
}
