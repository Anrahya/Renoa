//! Durable execution lifetime, separate from webhook admission and model state.
use super::{GitHubReviewError, GitHubReviewOutcome, GitHubReviewRun, catalog, runs, store};
use crate::{LocalHost, LocalHostError};
use rusqlite::{OptionalExtension as _, Transaction, params};
use serde::{Deserialize, Serialize};
use std::path::Path;
use uuid::Uuid;

pub const REVIEW_LIFETIME_MS: i64 = 60 * 60 * 1000;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct GitHubReviewWork {
    pub request_id: Uuid,
    pub deadline_at_ms: Option<i64>,
    pub finished: bool,
    pub publish_after_ms: i64,
    pub started_at_ms: Option<i64>,
    pub retry_after_ms: i64,
    pub last_error: Option<String>,
}

pub(super) fn initialize(tx: &Transaction<'_>) -> Result<(), catalog::HostCatalogError> {
    tx.execute_batch(
        "CREATE TABLE IF NOT EXISTS host_review_jobs (
        request_id TEXT PRIMARY KEY REFERENCES host_review_requests(id),
        deadline_at_ms INTEGER NOT NULL CHECK(deadline_at_ms>=0),
        publish_after_ms INTEGER NOT NULL DEFAULT 0 CHECK(publish_after_ms>=0)
    ) STRICT;",
    )?;
    let columns = tx
        .prepare("PRAGMA table_info(host_review_jobs)")?
        .query_map([], |row| row.get::<_, String>(1))?
        .collect::<Result<Vec<_>, _>>()?;
    if !columns.iter().any(|name| name == "started_at_ms") {
        tx.execute_batch(
            "ALTER TABLE host_review_jobs ADD COLUMN started_at_ms INTEGER;
            ALTER TABLE host_review_jobs ADD COLUMN retry_after_ms INTEGER NOT NULL DEFAULT 0;
            ALTER TABLE host_review_jobs ADD COLUMN last_error TEXT;
            UPDATE host_review_jobs SET started_at_ms=deadline_at_ms-3600000
              WHERE request_id IN (SELECT request_id FROM host_review_runs);",
        )?;
    }
    super::publication::initialize(tx)
}

pub(super) fn begin(path: &Path, id: Uuid, now: i64) -> Result<i64, GitHubReviewError> {
    let db = catalog::open_verified(path)?;
    let _ = store::get_request(&db, id)?;
    let deadline = now
        .checked_add(REVIEW_LIFETIME_MS)
        .filter(|_| now >= 0)
        .ok_or_else(|| GitHubReviewError::Invalid("invalid review start time".to_owned()))?;
    db.execute(
        "INSERT INTO host_review_jobs(request_id,deadline_at_ms) VALUES(?1,?2) ON CONFLICT DO NOTHING",
        params![id.to_string(), deadline],
    )?;
    Ok(db.query_row(
        "SELECT deadline_at_ms FROM host_review_jobs WHERE request_id=?1",
        [id.to_string()],
        |row| row.get(0),
    )?)
}

impl LocalHost {
    /// Lists admitted work and finished results awaiting GitHub publication.
    /// # Errors
    /// Returns catalog errors; this operation does not start a model.
    pub async fn github_review_work(&self) -> Result<Vec<GitHubReviewWork>, LocalHostError> {
        let path = self.config.database.clone();
        Ok(tokio::task::spawn_blocking(move || {
            let db = catalog::open_verified(&path)?;
            let mut query = db.prepare("SELECT r.id,j.deadline_at_ms,coalesce(x.terminal,0),coalesce(j.publish_after_ms,0),j.started_at_ms,coalesce(j.retry_after_ms,0),j.last_error
                FROM host_review_requests r LEFT JOIN host_review_jobs j ON j.request_id=r.id
                LEFT JOIN host_review_runs x ON x.request_id=r.id
                LEFT JOIN host_review_publications p ON p.request_id=r.id
                WHERE coalesce(x.terminal,0)=0 OR (j.request_id IS NOT NULL AND coalesce(p.settled,0)=0)
                ORDER BY r.sequence")?;
            let rows = query.query_map([], |row| {
                let raw: String = row.get(0)?;
                let request_id = Uuid::parse_str(&raw).map_err(|e| rusqlite::Error::FromSqlConversionFailure(
                    0, rusqlite::types::Type::Text, Box::new(e)))?;
                Ok(GitHubReviewWork { request_id, deadline_at_ms: row.get(1)?, finished: row.get(2)?, publish_after_ms: row.get(3)?, started_at_ms:row.get(4)?,retry_after_ms:row.get(5)?,last_error:row.get(6)? })
            })?.collect::<Result<Vec<_>, _>>()?;
            Ok::<_, GitHubReviewError>(rows)
        }).await??)
    }

    /// Persists the deadline before dispatch. Replays never extend the lifetime.
    /// # Errors
    /// Rejects an unknown request or invalid time.
    pub async fn begin_github_review(&self, id: Uuid, now: i64) -> Result<i64, LocalHostError> {
        let path = self.config.database.clone();
        Ok(tokio::task::spawn_blocking(move || begin(&path, id, now)).await??)
    }

    /// Records entry for the current attempt before preparation or inference.
    /// # Errors
    /// Rejects unknown jobs, invalid time or storage failure. Does not extend the deadline.
    pub async fn start_github_review(&self, id: Uuid, now: i64) -> Result<(), LocalHostError> {
        let path = self.config.database.clone();
        tokio::task::spawn_blocking(move || {
            if now < 0 { return Err(GitHubReviewError::Invalid("invalid worker start time".to_owned())); }
            let changed = catalog::open_verified(&path)?.execute(
                "UPDATE host_review_jobs SET started_at_ms=coalesce(started_at_ms,?2),retry_after_ms=0,last_error=NULL WHERE request_id=?1",
                params![id.to_string(),now])?;
            if changed == 0 { return Err(GitHubReviewError::NotFound); }
            Ok::<_,GitHubReviewError>(())
        }).await??;
        Ok(())
    }

    /// Saves retry timing and failure evidence for dispatch or per-job cleanup.
    /// # Errors
    /// Rejects unknown jobs, invalid time or storage failure.
    pub async fn defer_github_review(
        &self,
        id: Uuid,
        until: i64,
        reason: String,
    ) -> Result<(), LocalHostError> {
        let path = self.config.database.clone();
        tokio::task::spawn_blocking(move || {
            if until < 0 { return Err(GitHubReviewError::Invalid("invalid review retry time".to_owned())); }
            let changed = catalog::open_verified(&path)?.execute(
                "UPDATE host_review_jobs SET retry_after_ms=max(retry_after_ms,?2),last_error=?3 WHERE request_id=?1",
                params![id.to_string(),until,reason])?;
            if changed == 0 { return Err(GitHubReviewError::NotFound); }
            Ok::<_,GitHubReviewError>(())
        }).await??;
        Ok(())
    }

    /// Persists publication backoff independently of the completed model run.
    /// # Errors
    /// Rejects invalid timestamps, unknown jobs and storage failures.
    pub async fn defer_github_publication(
        &self,
        id: Uuid,
        until: i64,
    ) -> Result<(), LocalHostError> {
        let path = self.config.database.clone();
        Ok(tokio::task::spawn_blocking(move || {
            if until < 0 { return Err(GitHubReviewError::Invalid("invalid publication retry time".to_owned())); }
            let changed = catalog::open_verified(&path)?.execute(
                "UPDATE host_review_jobs SET publish_after_ms=max(publish_after_ms,?2) WHERE request_id=?1",
                params![id.to_string(),until],
            )?;
            if changed == 0 { return Err(GitHubReviewError::NotFound); }
            Ok::<_, GitHubReviewError>(())
        }).await??)
    }

    /// Called only after the worker's service has stopped. The execution lease
    /// prevents cleanup from deleting an active review's checkout.
    /// # Errors
    /// Returns ownership, cleanup or storage failures; never hides a live owner.
    pub async fn reap_github_review(&self, id: Uuid, now: i64) -> Result<(), LocalHostError> {
        let path = self.config.database.clone();
        tokio::task::spawn_blocking(move || {
            let _lease = crate::host::lease::ExecutionLease::acquire(&path.with_file_name(".reviews.lock"))?;
            let db = catalog::open_verified(&path)?;
            let _ = store::get_request(&db, id)?;
            let deadline: Option<i64> = db.query_row(
                "SELECT deadline_at_ms FROM host_review_jobs WHERE request_id=?1", [id.to_string()],
                |row| row.get(0)).optional().map_err(GitHubReviewError::from)?;
            for directory in ["review-workspaces", "github-executions"] {
                let root = path.with_file_name(directory).join(id.to_string());
                match std::fs::remove_dir_all(root) {
                    Ok(()) => {},
                    Err(e) if e.kind() == std::io::ErrorKind::NotFound => {},
                    Err(e) => return Err(e.into()),
                }
            }
            if deadline.is_none() { return Ok(()); }
            let (started, last_error): (Option<i64>, Option<String>) = db.query_row(
                "SELECT started_at_ms,last_error FROM host_review_jobs WHERE request_id=?1",
                [id.to_string()], |row| Ok((row.get(0)?,row.get(1)?))).map_err(GitHubReviewError::from)?;
            // ExecStopPost can run even when exec or credential loading failed.
            // A never-entered worker remains retryable inside its original lifetime.
            if started.is_none() && deadline.is_some_and(|v| now < v) { return Ok(()); }
            let snapshot = match runs::get(&db, id)? {
                Some(GitHubReviewRun::Finished { .. }) => return Ok(()),
                Some(GitHubReviewRun::Prepared { snapshot }) => Some(snapshot),
                None => None,
            };
            runs::save(&path, &GitHubReviewRun::Finished { request_id: id, snapshot,
                outcome: GitHubReviewOutcome::Incomplete { reason: if deadline.is_some_and(|v| now >= v) {
                    "Review exceeded its 60-minute lifetime; retained the transcript and stopped owned processes."
                } else {
                    "Review worker stopped before producing a complete result; retained its transcript."
                }.to_owned() + &last_error.map_or_else(String::new, |error| format!(" Last failure: {error}")) } })?;
            Ok::<_, LocalHostError>(())
        }).await?
    }
}
