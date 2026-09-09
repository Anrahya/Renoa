use rusqlite::{Connection, OptionalExtension as _};
use serde::Serialize;
use uuid::Uuid;

use super::review_activity::{
    self, ObservedPublicationState, ObservedReviewExecution, ObservedReviewPublication,
};
use super::{HostCatalogError, parse_id};

#[derive(Debug, Serialize)]
pub struct ObservedReview {
    pub request_id: Uuid,
    pub agent_id: Uuid,
    pub repository: String,
    pub pull_number: i64,
    pub admitted_at_ms: i64,
    pub reported_head_sha: String,
    pub reviewed_head_sha: Option<String>,
    /// Execution outcome only. `Reviewed` does not imply successful publication.
    pub state: ObservedReviewState,
    pub publication: ObservedPublicationState,
    pub worker_error: bool,
    pub retry_after_ms: Option<i64>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ObservedReviewState {
    Queued,
    Prepared,
    Reviewed,
    Skipped,
    Incomplete,
    Superseded,
}

#[derive(Debug, Serialize)]
pub struct ObservedReviewDetail {
    pub request_id: Uuid,
    pub provider: Option<String>,
    pub model: Option<String>,
    pub reasoning: Option<String>,
    pub reason: Option<String>,
    pub report: Option<crate::GitHubReviewReport>,
    /// Policy captured at admission, not proof of the exact triggering event.
    pub repository: crate::GitHubReviewRepository,
    pub execution: Option<ObservedReviewExecution>,
    pub publication: ObservedReviewPublication,
}

pub(super) fn detail(
    db: &Connection,
    request: Uuid,
) -> Result<Option<ObservedReviewDetail>, HostCatalogError> {
    let row=db.query_row("SELECT json_extract(r.record_json,'$.snapshot.provider'),
        json_extract(r.record_json,'$.snapshot.model'), json_extract(r.record_json,'$.snapshot.reasoning'),
        json_extract(r.record_json,'$.outcome.reason'), json_extract(r.record_json,'$.outcome.report'), q.repository_json
        FROM host_review_requests q LEFT JOIN host_review_runs r ON r.request_id=q.id WHERE q.id=?1",
        [request.to_string()],|r| Ok((r.get::<_,Option<String>>(0)?,r.get::<_,Option<String>>(1)?,
            r.get::<_,Option<String>>(2)?,r.get::<_,Option<String>>(3)?,r.get::<_,Option<String>>(4)?,r.get::<_,String>(5)?)))
        .optional()?;
    row.map(|(provider, model, reasoning, reason, report, repository)| {
        Ok(ObservedReviewDetail {
            request_id: request,
            provider,
            model,
            reasoning,
            reason,
            repository: review_activity::decode(&repository)?,
            execution: review_activity::execution(db, request)?,
            publication: review_activity::publication(db, request)?,
            report: report
                .map(|json| {
                    serde_json::from_str(&json).map_err(|error| {
                        HostCatalogError::Invalid(format!("invalid review report: {error}"))
                    })
                })
                .transpose()?,
        })
    })
    .transpose()
}

pub(super) fn read(db: &Connection) -> Result<Vec<ObservedReview>, HostCatalogError> {
    // Extract only summary facts inside SQLite; the frozen prompt, diff and
    // review context can be large and do not belong in an overview response.
    let mut q = db.prepare("SELECT q.id,
        json_extract(q.repository_json,'$.policy.agent_id'),
        json_extract(q.repository_json,'$.policy.full_name'),
        q.pull_number,q.admitted_at_ms,q.head_sha,
        json_extract(r.record_json,'$.snapshot.head_sha'),
        json_extract(r.record_json,'$.state'),json_extract(r.record_json,'$.outcome.status'),r.terminal,
        json_quote(coalesce(json_extract(p.record_json,'$.state'),'not_recorded')),
        j.last_error IS NOT NULL,j.retry_after_ms
        FROM host_review_requests q LEFT JOIN host_review_runs r ON r.request_id=q.id
        LEFT JOIN host_review_jobs j ON j.request_id=q.id
        LEFT JOIN host_review_publications p ON p.request_id=q.id
        ORDER BY q.sequence")?;
    let mut rows = q.query([])?;
    let mut items = Vec::new();
    while let Some(row) = rows.next()? {
        let run_phase: Option<String> = row.get(7)?;
        let outcome: Option<String> = row.get(8)?;
        let terminal: Option<bool> = row.get(9)?;
        let state = match (run_phase.as_deref(), outcome.as_deref(), terminal) {
            (None, None, None) => ObservedReviewState::Queued,
            (Some("prepared"), None, Some(false)) => ObservedReviewState::Prepared,
            (Some("finished"), Some("reviewed"), Some(true)) => ObservedReviewState::Reviewed,
            (Some("finished"), Some("skipped"), Some(true)) => ObservedReviewState::Skipped,
            (Some("finished"), Some("incomplete"), Some(true)) => ObservedReviewState::Incomplete,
            (Some("finished"), Some("superseded"), Some(true)) => ObservedReviewState::Superseded,
            _ => {
                return Err(HostCatalogError::Invalid(
                    "incompatible review observation state".to_owned(),
                ));
            }
        };
        items.push(ObservedReview {
            request_id: parse_id(&row.get::<_, String>(0)?)?,
            agent_id: parse_id(&row.get::<_, String>(1)?)?,
            repository: row.get(2)?,
            pull_number: row.get(3)?,
            admitted_at_ms: row.get(4)?,
            reported_head_sha: row.get(5)?,
            reviewed_head_sha: row.get(6)?,
            state,
            publication: review_activity::decode(&row.get::<_, String>(10)?)?,
            worker_error: row.get(11)?,
            retry_after_ms: row.get(12)?,
        });
    }
    Ok(items)
}
