use std::path::Path;

use renoa_agent::TokenUsage;
use rusqlite::{Connection, OptionalExtension as _, Transaction, params};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use super::{
    GitHubReviewError, GitHubReviewRequest, catalog, context::ReviewContext,
    findings::GitHubReviewReport, store,
};
use crate::{ModelProvider, ReasoningLevel};

/// Ownership is acquired before reading mutable policy or resumable state.
pub(super) struct OwnedReview {
    pub lease: crate::host::lease::ExecutionLease,
    pub request: GitHubReviewRequest,
    pub previous: Option<GitHubReviewRun>,
    pub policy: Option<super::GitHubReviewRepository>,
    pub automatic: bool,
}

pub(super) fn own(path: &Path, id: Uuid) -> Result<OwnedReview, crate::LocalHostError> {
    let lease = crate::host::lease::ExecutionLease::acquire(&path.with_file_name(".reviews.lock"))?;
    let db = catalog::open_verified(path)?;
    let request = store::get_request(&db, id)?;
    let previous = get(&db, id)?;
    let policy = store::repository(&db, request.repository.policy.repository_id)?;
    let automatic = db
        .query_row(
            "SELECT automatic_key IS NOT NULL FROM host_review_requests WHERE id=?1",
            [id.to_string()],
            |row| row.get(0),
        )
        .map_err(GitHubReviewError::from)?;
    Ok(OwnedReview {
        lease,
        request,
        previous,
        policy,
        automatic,
    })
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct GitHubReviewSnapshot {
    pub request: GitHubReviewRequest,
    pub base_sha: String,
    pub head_sha: String,
    pub provider: ModelProvider,
    pub model: String,
    pub reasoning: ReasoningLevel,
    pub prepared_at_ms: i64,
    pub(crate) model_spec: String,
    pub(crate) system_prompt: String,
    pub context: ReviewContext,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum GitHubReviewOutcome {
    Reviewed {
        report: GitHubReviewReport,
        usage: Option<TokenUsage>,
    },
    Skipped {
        reason: String,
    },
    Incomplete {
        reason: String,
    },
    Superseded {
        report: GitHubReviewReport,
        usage: Option<TokenUsage>,
    },
}

/// A prepared run owns one frozen input. Finished outcomes are immutable.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum GitHubReviewRun {
    Prepared {
        snapshot: Box<GitHubReviewSnapshot>,
    },
    Finished {
        request_id: Uuid,
        snapshot: Option<Box<GitHubReviewSnapshot>>,
        outcome: GitHubReviewOutcome,
    },
}

impl GitHubReviewRun {
    pub(super) fn request_id(&self) -> Uuid {
        match self {
            Self::Prepared { snapshot } => snapshot.request.id,
            Self::Finished { request_id, .. } => *request_id,
        }
    }
}

pub(super) fn initialize(tx: &Transaction<'_>) -> Result<(), catalog::HostCatalogError> {
    tx.execute_batch(
        "CREATE TABLE IF NOT EXISTS host_review_runs (
        request_id TEXT PRIMARY KEY REFERENCES host_review_requests(id),
        terminal INTEGER NOT NULL CHECK(terminal IN (0,1)),
        record_json TEXT NOT NULL CHECK(json_valid(record_json))
    ) STRICT;",
    )?;
    Ok(())
}

pub(super) fn get(db: &Connection, id: Uuid) -> Result<Option<GitHubReviewRun>, GitHubReviewError> {
    Ok(db
        .query_row(
            "SELECT record_json FROM host_review_runs WHERE request_id=?1",
            [id.to_string()],
            |row| store::json(row, 0),
        )
        .optional()?)
}

// The caller holds the Host's review process lease through preparation, model
// execution and this commit. Terminal results can only replay, never be edited.
pub(super) fn save(path: &Path, run: &GitHubReviewRun) -> Result<(), GitHubReviewError> {
    let mut db = catalog::open_verified(path)?;
    let tx = db.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
    if let Some(previous) = get(&tx, run.request_id())? {
        if matches!(previous, GitHubReviewRun::Finished { .. }) {
            return if previous == *run {
                Ok(())
            } else {
                Err(GitHubReviewError::Conflict)
            };
        }
        if let (
            GitHubReviewRun::Prepared { snapshot: old },
            GitHubReviewRun::Finished { snapshot, .. },
        ) = (&previous, run)
        {
            if snapshot.as_ref() != Some(old) {
                return Err(GitHubReviewError::Conflict);
            }
        } else if previous != *run {
            return Err(GitHubReviewError::Conflict);
        }
    }
    tx.execute("INSERT INTO host_review_runs VALUES(?1,?2,?3) ON CONFLICT(request_id) DO UPDATE SET terminal=excluded.terminal,record_json=excluded.record_json", params![run.request_id().to_string(), matches!(run, GitHubReviewRun::Finished { .. }), serde_json::to_string(run)?])?;
    tx.commit()?;
    Ok(())
}
