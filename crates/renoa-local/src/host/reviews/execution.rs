use renoa_agent::{ContentBlock, StopReason};
use renoa_kernel::{CommandId, SessionId};
use tokio_util::sync::CancellationToken;
use url::Url;
use uuid::Uuid;

use super::{
    GitHubReviewError, GitHubReviewOutcome, GitHubReviewRequest, GitHubReviewRun,
    GitHubReviewSnapshot, catalog, context, findings, github::GitHub, reviewer, runs, store,
};
use crate::{
    LocalHost, LocalHostError, LocalSession, LocalTurnOutcome, TurnObservation,
    host::{discover_profile_models, initial_reasoning, require_model},
};

impl LocalHost {
    /// Executes one admitted review using a short-lived GitHub App JWT supplied
    /// by the trusted local caller. Mints a read-only, single-repository token;
    /// no credential is persisted in review state or exposed as a model tool.
    /// Completed work replays without GitHub or model access. One review process
    /// per Host owns the execution lease. Cancellation drains the active effect.
    /// # Errors
    /// Returns authentication, context, lease, storage or provider failures.
    /// Preparation failures remain retryable; terminal runs require a new request.
    pub async fn execute_github_review(
        &self,
        request_id: Uuid,
        app_jwt: &str,
        workspace: &crate::InspectionSandboxConfig,
        cancellation: CancellationToken,
    ) -> Result<GitHubReviewRun, LocalHostError> {
        let origin = Url::parse("https://api.github.com")
            .map_err(|error| GitHubReviewError::Invalid(error.to_string()))?;
        let deadline = self
            .begin_github_review(request_id, TurnObservation::now()?.unix_milliseconds())
            .await?;
        let remaining = deadline.saturating_sub(TurnObservation::now()?.unix_milliseconds());
        if remaining <= 0 {
            self.reap_github_review(request_id, TurnObservation::now()?.unix_milliseconds())
                .await?;
            let path = self.config.database.clone();
            return tokio::task::spawn_blocking(move || {
                runs::get(&catalog::open_verified(&path)?, request_id)?
                    .ok_or_else(|| GitHubReviewError::NotFound.into())
            })
            .await?;
        }
        let run = self.execute_review_in(
            request_id,
            app_jwt,
            cancellation.clone(),
            origin,
            Some(workspace),
        );
        tokio::pin!(run);
        tokio::select! {
            result = &mut run => result,
            () = tokio::time::sleep(std::time::Duration::from_millis(remaining.unsigned_abs())) => {
                cancellation.cancel();
                // Drain the active effect and its children. The systemd service
                // independently kills the cgroup if cooperative shutdown hangs.
                run.await
            }
        }
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

    async fn execute_review_in(
        &self,
        request_id: Uuid,
        app_jwt: &str,
        cancellation: CancellationToken,
        origin: Url,
        workspace: Option<&crate::InspectionSandboxConfig>,
    ) -> Result<GitHubReviewRun, LocalHostError> {
        super::active(&cancellation)?;
        let database = self.config.database.clone();
        let runs::OwnedReview {
            lease,
            request,
            previous,
            policy,
            automatic,
        } = tokio::task::spawn_blocking(move || runs::own(&database, request_id)).await??;
        let checkout_root = self
            .config
            .database
            .with_file_name("review-workspaces")
            .join(request_id.to_string());
        // Reap a crashed owner's environment even when changed policy or a
        // closed PR will skip the recovered review before inference.
        if workspace.is_some() {
            super::checkout::Checkout::remove_abandoned(&checkout_root).await?;
        }
        if let Some(run @ GitHubReviewRun::Finished { .. }) = previous {
            return Ok(run);
        }
        if policy.as_ref() != Some(&request.repository) {
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
                .await;
        }
        let github =
            GitHub::connect(origin, app_jwt, &request.repository.policy, &cancellation).await?;
        let pull = github.pull(request.pull_number, &cancellation).await?;
        if pull.base.repo.as_ref().map(|repo| repo.id)
            != Some(request.repository.policy.repository_id)
        {
            return Err(GitHubReviewError::Authentication.into());
        }
        let snapshot = if let Some(GitHubReviewRun::Prepared { snapshot }) = previous {
            snapshot
        } else {
            if let Some(reason) = ineligible(&pull, &request, automatic) {
                return self
                    .finish_review(
                        request_id,
                        None,
                        GitHubReviewOutcome::Skipped {
                            reason: reason.to_owned(),
                        },
                    )
                    .await;
            }
            let context = match context::gather(&github, &pull, &cancellation).await {
                Ok(context) => context,
                Err(GitHubReviewError::ContextLimit) => return self.finish_review(request_id, None, incomplete("Repository context exceeds the bounded preparation limit; no model was called.")).await,
                Err(error) => return Err(error.into()),
            };
            self.prepare_review(
                request,
                pull.base.sha.clone(),
                pull.head.sha.clone(),
                context,
            )
            .await?
        };
        // A recovered run never substitutes fresh commits for its frozen input.
        if pull.state == "closed"
            || pull.base.sha != snapshot.base_sha
            || pull.head.sha != snapshot.head_sha
            || (pull.draft && snapshot.request.repository.policy.skip_drafts)
        {
            return self
                .finish_review(
                    request_id,
                    Some(snapshot),
                    GitHubReviewOutcome::Skipped {
                        reason: "Frozen review no longer matches the eligible open PR.".to_owned(),
                    },
                )
                .await;
        }
        let result = self
            .run_prepared_review(snapshot, github, workspace, cancellation, checkout_root)
            .await;
        drop(lease);
        result
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

    async fn prepare_review(
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
        let snapshot = Box::new(GitHubReviewSnapshot {
            request,
            base_sha,
            head_sha,
            provider,
            model: model.id().to_owned(),
            reasoning,
            prepared_at_ms: TurnObservation::now()?.unix_milliseconds(),
            model_spec: model.encoded_spec(),
            system_prompt: format!(
                "{}\n\nBatch at most {} tool calls in one response. The supplied patches already contain changed source. Prefer targeted ranges for missing callers, tests and surrounding context.\n\nHost-owned reviewer identity: {}\nHost-owned reviewer instructions:\n{}",
                reviewer::INSTRUCTIONS,
                reviewer::SOURCE_CALLS_PER_RESPONSE,
                recipe.recipe.name,
                recipe.recipe.instructions
            ),
            context,
        });
        self.save_review(GitHubReviewRun::Prepared {
            snapshot: snapshot.clone(),
        })
        .await?;
        Ok(snapshot)
    }

    async fn save_review(&self, run: GitHubReviewRun) -> Result<GitHubReviewRun, LocalHostError> {
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
            &serde_json::json!({"task":"Investigate defects introduced in this PR. Inspect callers, tests and surrounding source as needed. Complete the investigation, then return candidate findings in the required JSON schema.","base_sha":snapshot.base_sha,"head_sha":snapshot.head_sha,"context":snapshot.context}),
        )?;
        let candidate = self
            .review_stage(&session, snapshot, tools, false, prompt, &cancel)
            .await?;
        let candidates = match candidate {
            LocalTurnOutcome::Completed {
                output,
                stop_reason: StopReason::Stop,
            } => match findings::parse(&output) {
                Ok(report) => report,
                Err(_) => {
                    return Ok(incomplete(
                        "Investigator did not return a valid review report.",
                    ));
                }
            },
            LocalTurnOutcome::Failed { reason } => {
                return Ok(incomplete(&format!("Investigation failed: {reason}")));
            }
            _ => {
                return Ok(incomplete(
                    "Investigation stopped, failed or exhausted its budget; inspect the durable transcript.",
                ));
            }
        };
        let validation_prompt = serde_json::to_string(
            &serde_json::json!({"task":"Validate these candidates against pinned source, callers and tests. Seek counterexamples; discard unsupported claims and duplicates. Investigate as needed, then return final findings JSON in the same schema. Do not add new findings.","candidates":candidates}),
        )?;
        let validation = self
            .review_stage(&session, snapshot, tools, true, validation_prompt, &cancel)
            .await?;
        let mut report = match validation {
            LocalTurnOutcome::Completed {
                output,
                stop_reason: StopReason::Stop,
            } => match findings::parse(&output) {
                Ok(mut report) => {
                    report.findings.retain(|finding| {
                        candidates.findings.iter().any(|candidate| {
                            candidate.path == finding.path && candidate.line == finding.line
                        })
                    });
                    report.limitations.extend(candidates.limitations);
                    findings::validate(report, snapshot, tools, &cancel).await?
                }
                Err(_) => {
                    return Ok(incomplete(
                        "Validator did not return a valid bounded report.",
                    ));
                }
            },
            LocalTurnOutcome::Failed { reason } => {
                return Ok(incomplete(&format!("Validation failed: {reason}")));
            }
            _ => {
                return Ok(incomplete(
                    "Validation stopped, failed or exhausted its budget; inspect the durable transcript.",
                ));
            }
        };
        let history = session.history()?;
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

    async fn review_stage(
        &self,
        session: &LocalSession,
        snapshot: &GitHubReviewSnapshot,
        tools: &reviewer::ReviewTools<'_>,
        validation: bool,
        prompt: String,
        cancel: &CancellationToken,
    ) -> Result<LocalTurnOutcome, LocalHostError> {
        let id = if validation {
            let mut bytes = *snapshot.request.id.as_bytes();
            bytes[0] ^= 0x80;
            Uuid::from_bytes(bytes)
        } else {
            snapshot.request.id
        };
        let command = CommandId::from_uuid(id);
        let content = vec![ContentBlock::text(prompt)];
        if let Some(outcome) = session.replay_settled_turn(command, &content)? {
            return Ok(outcome);
        }
        let runtime = reviewer::runtime(&self.config, snapshot, tools).await?;
        Ok(session
            .execute_turn(command, content, &runtime, cancel.child_token())
            .await?)
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
