//! GitHub is a projection of an immutable Host result. An uncertain POST is
//! reconciled, never blindly retried: GitHub has no review idempotency key.
use super::{
    GitHubReviewError, GitHubReviewOutcome, GitHubReviewRun, catalog, github::GitHub, runs, store,
};
use crate::{LocalHost, LocalHostError};
use rusqlite::{OptionalExtension as _, Transaction, params};
use serde::{Deserialize, Serialize};
use std::path::Path;
use tokio_util::sync::CancellationToken;
use url::Url;
use uuid::Uuid;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum GitHubReviewPublication {
    Sending { payload: serde_json::Value },
    Published { review_id: i64, url: String },
    Suppressed { reason: String },
    NeedsAttention { reason: String },
}

impl GitHubReviewPublication {
    fn settled(&self) -> bool {
        !matches!(self, Self::Sending { .. })
    }
}

pub(super) fn initialize(tx: &Transaction<'_>) -> Result<(), catalog::HostCatalogError> {
    tx.execute_batch(
        "CREATE TABLE IF NOT EXISTS host_review_publications (
        request_id TEXT PRIMARY KEY REFERENCES host_review_requests(id),
        settled INTEGER NOT NULL CHECK(settled IN (0,1)),
        record_json TEXT NOT NULL CHECK(json_valid(record_json))
    ) STRICT;",
    )?;
    Ok(())
}

pub(super) fn get(
    path: &Path,
    id: Uuid,
) -> Result<Option<GitHubReviewPublication>, GitHubReviewError> {
    Ok(catalog::open_verified(path)?
        .query_row(
            "SELECT record_json FROM host_review_publications WHERE request_id=?1",
            [id.to_string()],
            |row| store::json(row, 0),
        )
        .optional()?)
}

fn save(path: &Path, id: Uuid, state: &GitHubReviewPublication) -> Result<(), GitHubReviewError> {
    catalog::open_verified(path)?.execute("INSERT INTO host_review_publications VALUES(?1,?2,?3)
        ON CONFLICT(request_id) DO UPDATE SET settled=excluded.settled,record_json=excluded.record_json",
        params![id.to_string(), !matches!(state, GitHubReviewPublication::Sending { .. }), serde_json::to_string(state)?])?;
    Ok(())
}

#[derive(Deserialize)]
struct RemoteReview {
    id: i64,
    body: String,
    commit_id: String,
    html_url: String,
    user: Reviewer,
}
#[derive(Deserialize)]
struct Reviewer {
    login: String,
}

impl LocalHost {
    /// Publishes or reconciles a finished review using a fresh App JWT. Only
    /// this deterministic adapter gets a write token; model tools remain read-only.
    /// # Errors
    /// Returns storage, ownership or API errors. An uncertain submission is not retried.
    pub async fn publish_github_review(
        &self,
        id: Uuid,
        jwt: &str,
        bot_login: &str,
        cancel: CancellationToken,
    ) -> Result<GitHubReviewPublication, LocalHostError> {
        self.publish_review_at(
            id,
            jwt,
            bot_login,
            Url::parse("https://api.github.com")
                .map_err(|e| GitHubReviewError::Invalid(e.to_string()))?,
            cancel,
        )
        .await
    }

    pub(super) async fn publish_review_at(
        &self,
        id: Uuid,
        jwt: &str,
        bot_login: &str,
        origin: Url,
        cancel: CancellationToken,
    ) -> Result<GitHubReviewPublication, LocalHostError> {
        let path = self.config.database.clone();
        let (lease, run, previous, request, policy) = tokio::task::spawn_blocking({
            let path = path.clone();
            move || {
                let lease = crate::host::lease::ExecutionLease::acquire(
                    &path.with_file_name(".review-publication.lock"),
                )?;
                let db = catalog::open_verified(&path)?;
                let request = store::get_request(&db, id)?;
                let policy = store::repository(&db, request.repository.policy.repository_id)?;
                Ok::<_, LocalHostError>((
                    lease,
                    runs::get(&db, id)?,
                    get(&path, id)?,
                    request,
                    policy,
                ))
            }
        })
        .await??;
        if let Some(state) = previous.as_ref().filter(|s| s.settled()) {
            return Ok(state.clone());
        }
        let Some(GitHubReviewRun::Finished {
            snapshot, outcome, ..
        }) = run
        else {
            return Err(
                GitHubReviewError::Invalid("review has no terminal outcome".to_owned()).into(),
            );
        };
        // Operational failures belong to the Host control plane. Do not even
        // acquire a GitHub credential for a new incomplete publication. A prior
        // uncertain POST must still be reconciled; it may already exist remotely.
        if matches!(outcome, GitHubReviewOutcome::Incomplete { .. })
            && !matches!(previous, Some(GitHubReviewPublication::Sending { .. }))
        {
            let state = GitHubReviewPublication::Suppressed {
                reason: "Incomplete review; diagnostics retained by the Host.".to_owned(),
            };
            self.save_publication(id, state.clone()).await?;
            return Ok(state);
        }
        let sha = snapshot
            .as_ref()
            .map_or(&request.reported_head_sha, |s| &s.head_sha);
        let marker = format!("<!-- renoa-review:{id} -->");
        let github = GitHub::connect_with_permissions(
            origin,
            jwt,
            &request.repository.policy,
            true,
            &cancel,
        )
        .await?;
        // Resolve a prior lost acknowledgement before considering current policy
        // or head. Its already-submitted review remains bound to the frozen SHA.
        if let Some(GitHubReviewPublication::Sending { payload }) = previous {
            let reconciled =
                reconcile(&github, request.pull_number, &payload, bot_login, &cancel).await?;
            let state = reconciled.unwrap_or_else(|| GitHubReviewPublication::NeedsAttention {
                reason: "GitHub submission outcome is unknown. No matching review was found; automatic reposting is disabled to prevent duplicates.".to_owned(),
            });
            self.save_publication(id, state.clone()).await?;
            return Ok(state);
        }
        let pull = github.pull(request.pull_number, &cancel).await?;
        let suppressed = if policy.as_ref() != Some(&request.repository) {
            Some("Repository policy changed before publication.")
        } else if pull.state != "open"
            || pull.head.sha != *sha
            || snapshot
                .as_ref()
                .is_some_and(|s| s.base_sha != pull.base.sha)
        {
            Some("PR closed or changed before publication; retained the Host result.")
        } else if matches!(
            outcome,
            GitHubReviewOutcome::Skipped { .. } | GitHubReviewOutcome::Superseded { .. }
        ) {
            Some("Review was skipped or superseded.")
        } else {
            None
        };
        if let Some(reason) = suppressed {
            let state = GitHubReviewPublication::Suppressed {
                reason: reason.to_owned(),
            };
            self.save_publication(id, state.clone()).await?;
            return Ok(state);
        }
        let payload = payload(&outcome, sha, &marker);
        self.save_publication(
            id,
            GitHubReviewPublication::Sending {
                payload: payload.clone(),
            },
        )
        .await?;
        let state = post_review(&github, request.pull_number, &payload, bot_login, &cancel).await?;
        self.save_publication(id, state.clone()).await?;
        drop(lease);
        Ok(state)
    }

    async fn save_publication(
        &self,
        id: Uuid,
        state: GitHubReviewPublication,
    ) -> Result<(), LocalHostError> {
        let path = self.config.database.clone();
        tokio::task::spawn_blocking(move || save(&path, id, &state)).await??;
        Ok(())
    }
}

async fn post_review(
    github: &GitHub,
    number: i64,
    payload: &serde_json::Value,
    bot: &str,
    cancel: &CancellationToken,
) -> Result<GitHubReviewPublication, GitHubReviewError> {
    let number = number.to_string();
    let parts: Vec<_> = std::iter::once("repos")
        .chain(github.repository.split('/'))
        .chain(["pulls", &number, "reviews"])
        .collect();
    // Sending is durable before this call. Errors leave it for read-back,
    // including timeouts after GitHub has already committed the review.
    let bytes = github
        .request(
            reqwest::Method::POST,
            &parts,
            &[],
            Some(serde_json::to_vec(payload)?),
            false,
            cancel,
        )
        .await?;
    let remote: RemoteReview = serde_json::from_slice(&bytes)?;
    if remote.user.login != bot
        || remote.commit_id != payload["commit_id"]
        || remote.body != payload["body"]
    {
        return Err(GitHubReviewError::Invalid(
            "GitHub returned a different review identity".to_owned(),
        ));
    }
    Ok(GitHubReviewPublication::Published {
        review_id: remote.id,
        url: remote.html_url,
    })
}

async fn reconcile(
    github: &GitHub,
    number: i64,
    payload: &serde_json::Value,
    bot: &str,
    cancel: &CancellationToken,
) -> Result<Option<GitHubReviewPublication>, GitHubReviewError> {
    let mut page = 1_u64;
    loop {
        let reviews: Vec<RemoteReview> = github
            .repo_json(
                &["pulls", &number.to_string(), "reviews"],
                &[("per_page", "100"), ("page", &page.to_string())],
                cancel,
            )
            .await?;
        if let Some(review) = reviews.iter().find(|r| {
            r.user.login == bot && r.commit_id == payload["commit_id"] && r.body == payload["body"]
        }) {
            return Ok(Some(GitHubReviewPublication::Published {
                review_id: review.id,
                url: review.html_url.clone(),
            }));
        }
        if reviews.len() < 100 {
            return Ok(None);
        }
        page = page.checked_add(1).ok_or(GitHubReviewError::ContextLimit)?;
    }
}

fn payload(outcome: &GitHubReviewOutcome, sha: &str, marker: &str) -> serde_json::Value {
    let (summary, comments) = match outcome {
        GitHubReviewOutcome::Reviewed { report, .. } => {
            let mut summary = format!(
                "Soundwave reviewed commit `{sha}`. {} validated finding(s).{}",
                report.findings.len(),
                if report.limitations.is_empty() {
                    String::new()
                } else {
                    format!(
                        "\n\nCoverage and verification limits apply; no findings is not proof of correctness.\n\n<details>\n<summary>Review limitations</summary>\n\n{}\n\n</details>",
                        report
                            .limitations
                            .iter()
                            .map(|s| format!(
                                "- {}",
                                s.replace('&', "&amp;")
                                    .replace('<', "&lt;")
                                    .replace('>', "&gt;")
                            ))
                            .collect::<Vec<_>>()
                            .join("\n")
                    )
                }
            );
            for finding in report.findings.iter().filter(|f| !f.in_diff) {
                use std::fmt::Write as _;
                write!(
                    &mut summary,
                    "\n\n{}\n\nSource: `{}:{}` ({:?} of the pinned comparison).",
                    finding_body(finding),
                    finding.path,
                    finding.line,
                    finding.side
                )
                .expect("writing to String cannot fail");
            }
            let comments: Vec<_> = report.findings.iter().filter(|f| f.in_diff).map(|f| serde_json::json!({
                "path": f.path, "line": f.line, "side":match f.side { crate::GitSide::Base => "LEFT", crate::GitSide::Head => "RIGHT" }, "body": finding_body(f)
            })).collect();
            (summary, comments)
        }
        GitHubReviewOutcome::Incomplete { reason } => (
            format!(
                "Soundwave could not complete this review.\n\n{reason}\n\nThis is not a clean review; the Host retained the execution record."
            ),
            Vec::new(),
        ),
        GitHubReviewOutcome::Skipped { reason } => (reason.clone(), Vec::new()),
        GitHubReviewOutcome::Superseded { .. } => ("Review superseded.".to_owned(), Vec::new()),
    };
    serde_json::json!({"commit_id":sha,"event":"COMMENT","body":format!("Soundwave reporting.\n\n{summary}\n\n{marker}"),"comments":comments})
}

fn finding_body(f: &super::GitHubReviewFinding) -> String {
    format!(
        "**[{:?}] {}**\n\n{}\n\n{}\n\n{}",
        f.priority.unwrap_or(super::ReviewPriority::P2),
        f.title,
        f.trigger,
        f.consequence,
        f.correction
    )
}
