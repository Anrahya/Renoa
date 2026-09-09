use renoa_kernel::SessionId;
use tokio_util::sync::CancellationToken;
use url::Url;
use uuid::Uuid;

#[cfg(test)]
mod git_fixture;

use super::{
    GitHubReviewError, GitHubReviewOutcome, GitHubReviewRequest, GitHubReviewRun,
    GitHubReviewSnapshot, catalog, findings, github::GitHub, reviewer, runs, store,
};
use crate::{LocalHost, LocalHostError, LocalSession};

enum PreparedReview {
    Ready {
        snapshot: Box<GitHubReviewSnapshot>,
        github: GitHub,
    },
    Finished(GitHubReviewRun),
}

impl LocalHost {
    /// Executes one admitted review using a short-lived GitHub App JWT supplied
    /// by the trusted local caller. Mints a read-only, single-repository token;
    /// no credential is persisted in review state or exposed as a model tool.
    /// Completed work replays without GitHub or model access. One review process
    /// per Host owns the execution lease. Cancellation drains the active effect.
    /// # Errors
    /// Returns authentication, context, lease, storage or provider failures.
    /// Recoverable preparation failures retain backoff and their cause; other
    /// worker failures become incomplete. Terminal runs require a new request.
    pub async fn execute_github_review(
        &self,
        request_id: Uuid,
        app_jwt: &str,
        workspace: &crate::InspectionSandboxConfig,
        cancellation: CancellationToken,
    ) -> Result<GitHubReviewRun, LocalHostError> {
        let origin = Url::parse("https://api.github.com")
            .map_err(|error| GitHubReviewError::Invalid(error.to_string()))?;
        self.execute_review_worker(request_id, app_jwt, Some(workspace), cancellation, origin)
            .await
    }

    #[cfg(test)]
    pub(super) async fn execute_review_at(
        &self,
        request_id: Uuid,
        app_jwt: &str,
        cancellation: CancellationToken,
        origin: Url,
    ) -> Result<GitHubReviewRun, LocalHostError> {
        self.execute_review_in(request_id, app_jwt, cancellation, origin, None)
            .await
    }

    pub(super) async fn execute_review_in(
        &self,
        request_id: Uuid,
        app_jwt: &str,
        cancellation: CancellationToken,
        origin: Url,
        workspace: Option<&crate::InspectionSandboxConfig>,
    ) -> Result<GitHubReviewRun, LocalHostError> {
        super::active(&cancellation)?;
        let database = self.config.database.clone();
        let mut owned =
            tokio::task::spawn_blocking(move || runs::own(&database, request_id)).await??;
        let checkout_root = self
            .config
            .database
            .with_file_name("review-workspaces")
            .join(request_id.to_string());
        let root = workspace.map(|_| checkout_root.as_path());
        if let Some(root) = root {
            super::checkout::Checkout::remove_abandoned(root).await?;
        }
        let preparation = self
            .prepare_owned_review(&mut owned, app_jwt, &cancellation, origin, root)
            .await;
        let result = match preparation {
            Ok(PreparedReview::Ready { snapshot, github }) => {
                self.run_prepared_review(
                    snapshot,
                    github,
                    workspace,
                    cancellation,
                    checkout_root.clone(),
                )
                .await
            }
            Ok(PreparedReview::Finished(run)) => Ok(run),
            Err(error) => Err(error),
        };
        // Keep the lease through cleanup, including policy changes and failed
        // preparation. A replacement worker must never race removal of its files.
        if let Some(root) = root {
            super::checkout::Checkout::remove_abandoned(root).await?;
        }
        drop(owned.lease);
        result
    }

    async fn prepare_owned_review(
        &self,
        owned: &mut runs::OwnedReview,
        app_jwt: &str,
        cancellation: &CancellationToken,
        origin: Url,
        root: Option<&std::path::Path>,
    ) -> Result<PreparedReview, LocalHostError> {
        let previous = owned.previous.take();
        let request = &owned.request;
        let request_id = request.id;
        if let Some(run @ GitHubReviewRun::Finished { .. }) = previous {
            return Ok(PreparedReview::Finished(run));
        }
        if owned.policy.as_ref() != Some(&request.repository) {
            let snapshot = match previous {
                Some(GitHubReviewRun::Prepared { snapshot }) => Some(snapshot),
                _ => None,
            };
            return self
                .finish_review(
                    request_id,
                    snapshot,
                    GitHubReviewOutcome::Skipped {
                        reason: "Repository policy changed after admission.".to_owned(),
                    },
                )
                .await
                .map(PreparedReview::Finished);
        }
        let github =
            GitHub::connect(origin, app_jwt, &request.repository.policy, cancellation).await?;
        let pull = github.pull(request.pull_number, cancellation).await?;
        if pull.base.repo.as_ref().map(|repo| repo.id)
            != Some(request.repository.policy.repository_id)
        {
            return Err(GitHubReviewError::Authentication.into());
        }
        let snapshot = if let Some(GitHubReviewRun::Prepared { snapshot }) = previous {
            snapshot
        } else {
            if let Some(reason) = ineligible(&pull, request, owned.automatic) {
                return self
                    .finish_review(
                        request_id,
                        None,
                        GitHubReviewOutcome::Skipped {
                            reason: reason.to_owned(),
                        },
                    )
                    .await
                    .map(PreparedReview::Finished);
            }
            self.prepare_review_source(request.clone(), &pull, &github, root, cancellation)
                .await?
        };
        // Recheck after checkout; preparation may have taken long enough for
        // another push. A recovered run also keeps its original frozen commits.
        let current = github.pull(request.pull_number, cancellation).await?;
        if current.state == "closed"
            || current.base.sha != snapshot.base_sha
            || current.head.sha != snapshot.head_sha
            || (current.draft && snapshot.request.repository.policy.skip_drafts)
        {
            return self
                .finish_review(
                    request_id,
                    Some(snapshot),
                    GitHubReviewOutcome::Skipped {
                        reason: "Frozen review no longer matches the eligible open PR.".to_owned(),
                    },
                )
                .await
                .map(PreparedReview::Finished);
        }
        Ok(PreparedReview::Ready { snapshot, github })
    }

    async fn run_prepared_review(
        &self,
        snapshot: Box<GitHubReviewSnapshot>,
        github: GitHub,
        workspace: Option<&crate::InspectionSandboxConfig>,
        cancellation: CancellationToken,
        checkout_root: std::path::PathBuf,
    ) -> Result<GitHubReviewRun, LocalHostError> {
        let request_id = snapshot.request.id;
        let checkout = match workspace {
            Some(config) => Some(
                super::checkout::Checkout::prepare(
                    checkout_root,
                    config,
                    &snapshot,
                    &github,
                    &cancellation,
                )
                .await?,
            ),
            None => None,
        };
        let source = match &checkout {
            Some(checkout) => reviewer::ReviewToolSource::Sandbox(&checkout.sandbox),
            #[cfg(test)]
            None => reviewer::ReviewToolSource::Fixture(&snapshot),
            #[cfg(not(test))]
            None => {
                return Err(LocalHostError::Configuration(
                    "review requires an inspection sandbox".to_owned(),
                ));
            }
        };
        let tools = reviewer::ReviewTools {
            github: &github,
            source,
        };
        let outcome = self.investigate(&snapshot, &tools, cancellation).await;
        let result = match outcome {
            Ok(outcome) => {
                self.finish_review(request_id, Some(snapshot), outcome)
                    .await
            }
            Err(error) => Err(error),
        };
        if let Some(checkout) = checkout {
            checkout.close().await?;
        }
        result
    }

    pub(super) async fn save_review(
        &self,
        run: GitHubReviewRun,
    ) -> Result<GitHubReviewRun, LocalHostError> {
        let database = self.config.database.clone();
        tokio::task::spawn_blocking(move || {
            runs::save(&database, &run)?;
            Ok::<_, LocalHostError>(run)
        })
        .await?
    }

    async fn finish_review(
        &self,
        request_id: Uuid,
        snapshot: Option<Box<GitHubReviewSnapshot>>,
        outcome: GitHubReviewOutcome,
    ) -> Result<GitHubReviewRun, LocalHostError> {
        self.save_review(GitHubReviewRun::Finished {
            request_id,
            snapshot,
            outcome,
        })
        .await
    }

    async fn investigate(
        &self,
        snapshot: &GitHubReviewSnapshot,
        tools: &reviewer::ReviewTools<'_>,
        cancel: CancellationToken,
    ) -> Result<GitHubReviewOutcome, LocalHostError> {
        let root = self
            .config
            .database
            .with_file_name("review-sessions")
            .join(snapshot.request.id.to_string());
        let agent = snapshot.request.repository.policy.agent_id;
        let session_id = SessionId::from_uuid(snapshot.request.id);
        let session = tokio::task::spawn_blocking(move || {
            std::fs::create_dir_all(&root)?;
            Ok::<_, LocalHostError>(LocalSession::create(
                root.join("kernel.sqlite"),
                agent,
                session_id,
            )?)
        })
        .await??;
        let prompt = serde_json::to_string(
            &serde_json::json!({"task":"Investigate defects introduced in this PR. Inspect callers, tests and surrounding source as needed. Complete the investigation, then return candidate findings in the required JSON schema.","base_sha":snapshot.base_sha,"head_sha":snapshot.head_sha,"context":snapshot.context.prompt()?}),
        )?;
        let candidates = match self
            .review_report(&session, snapshot, tools, false, prompt, &cancel)
            .await?
        {
            super::stages::ReportStageResult::Complete(report) => report,
            super::stages::ReportStageResult::Incomplete(reason) => return Ok(incomplete(&reason)),
        };
        let validation_prompt = serde_json::to_string(
            &serde_json::json!({"task":"Validate these candidates against pinned source, callers and tests. Independently reconstruct each trigger and seek a counterexample. Challenge the proposed correction as well as the defect: does it preserve durable identity, deadlines, ownership, permissions and crash recovery? Correct unsafe remedies or describe the required behavior without inventing an implementation. Calibrate severity to demonstrated impact and available recovery. Discard unsupported claims and duplicates. Investigate as needed, then return final findings JSON in the same schema. Do not add new findings.","candidates":candidates}),
        )?;
        let mut report = match self
            .review_report(&session, snapshot, tools, true, validation_prompt, &cancel)
            .await?
        {
            super::stages::ReportStageResult::Complete(report) => report,
            super::stages::ReportStageResult::Incomplete(reason) => return Ok(incomplete(&reason)),
        };
        report.findings.retain(|finding| {
            candidates.findings.iter().any(|candidate| {
                candidate.path == finding.path
                    && candidate.line == finding.line
                    && candidate.side == finding.side
            })
        });
        report.limitations.extend(candidates.limitations);
        let mut report = findings::validate(report, snapshot, tools, &cancel).await?;
        let history = session.history()?;
        if let Some(limitation) = snapshot.context.inventory_limitation(
            &snapshot.head_sha,
            history.iter().filter_map(|entry| match &entry.message {
                renoa_agent::Message::Tool { result } => Some(result),
                _ => None,
            }),
        ) {
            report.limitations.push(limitation);
        }
        if history.iter().any(|entry| matches!(&entry.message, renoa_agent::Message::Tool { result } if result.is_error)) {
            report.limitations.push("At least one source lookup failed; inspect the durable transcript for missing context.".to_owned());
        }
        let usage = session.recorded_token_usage()?;
        self.classify_review(snapshot, tools.github, report, usage, &cancel)
            .await
    }

    async fn classify_review(
        &self,
        snapshot: &GitHubReviewSnapshot,
        github: &GitHub,
        report: super::GitHubReviewReport,
        usage: Option<renoa_agent::TokenUsage>,
        cancel: &CancellationToken,
    ) -> Result<GitHubReviewOutcome, LocalHostError> {
        let current = github.pull(snapshot.request.pull_number, cancel).await?;
        let database = self.config.database.clone();
        let repository_id = snapshot.request.repository.policy.repository_id;
        let policy = tokio::task::spawn_blocking(move || {
            let db = catalog::open_verified(&database)?;
            store::repository(&db, repository_id)
        })
        .await??;
        if current.state == "closed"
            || current.head.sha != snapshot.head_sha
            || current.base.sha != snapshot.base_sha
            || (current.draft && snapshot.request.repository.policy.skip_drafts)
            || policy.as_ref() != Some(&snapshot.request.repository)
        {
            Ok(GitHubReviewOutcome::Superseded { report, usage })
        } else {
            Ok(GitHubReviewOutcome::Reviewed { report, usage })
        }
    }
}

fn ineligible(
    pull: &super::github::Pull,
    request: &GitHubReviewRequest,
    automatic: bool,
) -> Option<&'static str> {
    if automatic && request.reported_head_sha != pull.head.sha {
        Some("A newer PR commit superseded this queued automatic review.")
    } else if pull.state == "closed" || (pull.draft && request.repository.policy.skip_drafts) {
        Some("PR is closed or excluded as a draft.")
    } else {
        None
    }
}

fn incomplete(reason: &str) -> GitHubReviewOutcome {
    GitHubReviewOutcome::Incomplete {
        reason: reason.to_owned(),
    }
}
