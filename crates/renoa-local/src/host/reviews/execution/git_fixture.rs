//! Deterministic repository transport boundary; the real tools, model bridge,
//! agent loop, validation, durable outcomes and publisher run unchanged.
use super::*;
use crate::host::reviews::context;

impl LocalHost {
    pub(in crate::host::reviews) async fn execute_git_at(
        &self,
        id: Uuid,
        root: &std::path::Path,
        origin: Url,
    ) -> Result<GitHubReviewRun, LocalHostError> {
        let path = self.config.database.clone();
        let owned = tokio::task::spawn_blocking(move || runs::own(&path, id)).await??;
        if let Some(run @ GitHubReviewRun::Finished { .. }) = owned.previous {
            return Ok(run);
        }
        let cancel = CancellationToken::new();
        let github = GitHub::connect(
            origin,
            "private-app-jwt",
            &owned.request.repository.policy,
            &cancel,
        )
        .await?;
        let pull = github.pull(owned.request.pull_number, &cancel).await?;
        let mut context = context::gather(&github, &pull, &cancel).await?;
        context
            .load_inventory(root, &pull.head.sha, &cancel)
            .await?;
        let snapshot = self
            .prepare_review(owned.request, pull.base.sha, pull.head.sha, context)
            .await?;
        let workspace = crate::LocalWorkspace::open(root)?;
        let tools = reviewer::ReviewTools {
            github: &github,
            source: reviewer::ReviewToolSource::Workspace(&workspace),
        };
        let outcome = self.investigate(&snapshot, &tools, cancel).await?;
        self.finish_review(id, Some(snapshot), outcome).await
    }
}
