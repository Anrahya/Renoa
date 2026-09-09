//! Host-owned GitHub review admission and bounded execution, outside RCP.
use std::collections::BTreeSet;

use renoa_kernel::AgentId;
use serde::{Deserialize, Serialize};
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use super::{LocalHost, LocalHostError, catalog};

mod checkout;
mod context;
mod control;
mod execution;
mod findings;
mod github;
mod jobs;
mod publication;
mod reviewer;
mod runs;
mod stages;
mod store;
#[cfg(test)]
mod tests;
mod webhook;
mod worker;

pub use context::{ReviewCheck, ReviewContext, ReviewFile};
pub use control::{HostReviewControl, ReviewPolicyUpdate};
pub use findings::{GitHubReviewEvidence, GitHubReviewFinding, GitHubReviewReport, ReviewPriority};
pub use jobs::{GitHubReviewWork, REVIEW_LIFETIME_MS};
pub use publication::GitHubReviewPublication;
pub use runs::{GitHubReviewOutcome, GitHubReviewRun, GitHubReviewSnapshot};
pub(super) use store::initialize;
pub use webhook::GitHubReviewWebhook;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GitHubReviewTrigger {
    Opened,
    Reopened,
    ReadyForReview,
    Synchronize,
}

/// Repository IDs, rather than mutable names, define the subscription boundary.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GitHubReviewPolicy {
    pub repository_id: i64,
    pub installation_id: i64,
    pub full_name: String,
    pub agent_id: AgentId,
    pub enabled: bool,
    pub triggers: BTreeSet<GitHubReviewTrigger>,
    pub skip_drafts: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct GitHubReviewRepository {
    pub revision: i64,
    pub policy: GitHubReviewPolicy,
}

/// A request is not an execution. Reported commits must be reconciled with
/// GitHub before an executor freezes its actual base and head.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct GitHubReviewRequest {
    pub sequence: i64,
    pub id: Uuid,
    pub repository: GitHubReviewRepository,
    pub pull_number: i64,
    pub reported_base_sha: String,
    pub reported_head_sha: String,
    pub admitted_at_ms: i64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GitHubReviewSkip {
    UnsupportedEvent,
    UnconfiguredRepository,
    Disabled,
    TriggerDisabled,
    Draft,
    Closed,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum GitHubReviewAdmission {
    Queued { request_id: Uuid },
    Ignored { reason: GitHubReviewSkip },
}

/// Trusted local management input. Remote surfaces must authenticate and bind
/// their principal to this Host before calling these operations.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "action", rename_all = "snake_case", deny_unknown_fields)]
pub enum GitHubReviewCommand {
    SetRepository {
        operation_id: Uuid,
        /// None creates a subscription; edits require the current revision.
        expected_revision: Option<i64>,
        policy: GitHubReviewPolicy,
    },
    Request {
        operation_id: Uuid,
        repository_id: i64,
        pull_number: i64,
        reported_base_sha: String,
        reported_head_sha: String,
    },
    Repositories {
        after: Option<i64>,
    },
    Requests {
        after: i64,
    },
    Run {
        request_id: Uuid,
    },
    Publication {
        request_id: Uuid,
    },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum GitHubReviewReply {
    Repository {
        record: GitHubReviewRepository,
    },
    Request {
        record: GitHubReviewRequest,
    },
    Repositories {
        records: Vec<GitHubReviewRepository>,
    },
    Requests {
        records: Vec<GitHubReviewRequest>,
    },
    Run {
        record: Option<GitHubReviewRun>,
    },
    Publication {
        record: Option<GitHubReviewPublication>,
    },
}

#[derive(Debug, thiserror::Error)]
pub enum GitHubReviewError {
    #[error("review workspace failed: {0}")]
    Workspace(#[from] std::io::Error),
    #[error("invalid GitHub review request: {0}")]
    Invalid(String),
    #[error("GitHub review operation or repository revision conflicts")]
    Conflict,
    #[error("GitHub review repository not configured")]
    NotFound,
    #[error("GitHub review inbox is full; admission remains unacknowledged")]
    Capacity,
    #[error("GitHub review webhook authentication failed")]
    Authentication,
    #[error("this principal does not own the configured Host")]
    Forbidden,
    #[error("GitHub review operation cancelled")]
    Cancelled,
    #[error("GitHub review HTTP request failed: {0}")]
    Http(#[from] reqwest::Error),
    #[error("GitHub review API returned HTTP {status}; retry-after: {retry_after:?}")]
    Api {
        status: u16,
        retry_after: Option<String>,
    },
    #[error("GitHub review context exceeds its bounded input limit")]
    ContextLimit,
    #[error("GitHub PR changed while gathering context; retry preparation")]
    MovingPull,
    #[error("GitHub review runtime: {0}")]
    Runtime(#[from] renoa_agent_loop::AgentLoopBuildError),
    #[error(transparent)]
    Database(#[from] rusqlite::Error),
    #[error(transparent)]
    Catalog(#[from] catalog::HostCatalogError),
    #[error(transparent)]
    Json(#[from] serde_json::Error),
}

impl LocalHost {
    /// Checks the durable admission receipt before requesting webhook redelivery.
    /// # Errors
    /// Returns Host catalog errors without contacting GitHub.
    pub async fn has_github_delivery(&self, id: Uuid) -> Result<bool, LocalHostError> {
        let database = self.config.database.clone();
        Ok(tokio::task::spawn_blocking(move || {
            catalog::open_verified(&database)?
                .query_row(
                    "SELECT EXISTS(SELECT 1 FROM host_review_deliveries WHERE delivery_id=?1)",
                    [id.to_string()],
                    |row| row.get(0),
                )
                .map_err(GitHubReviewError::from)
        })
        .await??)
    }

    /// Applies local review management with durable receipts and revision checks.
    /// Listing is bounded to 20 records; requests are ordered by admission sequence.
    /// # Errors
    /// Rejects invalid policies, unknown agents, conflicting retries and stale edits.
    pub async fn manage_github_review(
        &self,
        command: GitHubReviewCommand,
        now_ms: i64,
        cancellation: CancellationToken,
    ) -> Result<GitHubReviewReply, LocalHostError> {
        let database = self.config.database.clone();
        Ok(tokio::task::spawn_blocking(move || {
            store::manage(&database, &command, now_ms, &cancellation)
        })
        .await??)
    }

    /// Authenticates raw GitHub bytes and persists admission before returning.
    /// No model is called, and webhook arrival order never supersedes a request.
    /// # Errors
    /// Rejects malformed signatures/payloads, wrong installations, and conflicting replay.
    pub async fn admit_github_review_webhook(
        &self,
        webhook: GitHubReviewWebhook<'_>,
        secret: &[u8],
        now_ms: i64,
        cancellation: CancellationToken,
    ) -> Result<GitHubReviewAdmission, LocalHostError> {
        active(&cancellation)?;
        let delivery = webhook::authenticate(webhook, secret)?;
        let database = self.config.database.clone();
        Ok(tokio::task::spawn_blocking(move || {
            store::admit(&database, &delivery, now_ms, &cancellation)
        })
        .await??)
    }
}

fn active(cancellation: &CancellationToken) -> Result<(), GitHubReviewError> {
    if cancellation.is_cancelled() {
        Err(GitHubReviewError::Cancelled)
    } else {
        Ok(())
    }
}

fn validate_target(number: i64, base: &str, head: &str) -> Result<(), GitHubReviewError> {
    let sha = |value: &str| {
        value.len() == 40
            && value
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    };
    if number <= 0 || !sha(base) || !sha(head) {
        return Err(GitHubReviewError::Invalid(
            "provide a positive PR number and lowercase 40-character commit SHAs".to_owned(),
        ));
    }
    Ok(())
}
