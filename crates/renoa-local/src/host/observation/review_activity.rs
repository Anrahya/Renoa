//! Execution and delivery metadata, without frozen inputs or pending POST bodies.
use rusqlite::{Connection, OptionalExtension as _};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use super::HostCatalogError;

#[derive(Clone, Copy, Debug, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ObservedPublicationState {
    NotRecorded,
    Sending,
    Published,
    Suppressed,
    NeedsAttention,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct ObservedReviewExecution {
    pub started_at_ms: Option<i64>,
    pub deadline_at_ms: i64,
    pub retry_after_ms: i64,
    pub publish_after_ms: i64,
    pub last_error: Option<String>,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum ObservedReviewPublication {
    NotRecorded,
    Sending,
    Published { review_id: i64, url: String },
    Suppressed { reason: String },
    NeedsAttention { reason: String },
}

pub(super) fn decode<T: serde::de::DeserializeOwned>(json: &str) -> Result<T, HostCatalogError> {
    serde_json::from_str(json)
        .map_err(|error| HostCatalogError::Invalid(format!("invalid review metadata: {error}")))
}

pub(super) fn execution(
    db: &Connection,
    request: Uuid,
) -> Result<Option<ObservedReviewExecution>, HostCatalogError> {
    let json: Option<String> = db
        .query_row(
            "SELECT json_object('started_at_ms',started_at_ms,'deadline_at_ms',deadline_at_ms,
        'retry_after_ms',retry_after_ms,'publish_after_ms',publish_after_ms,'last_error',last_error)
        FROM host_review_jobs WHERE request_id=?1",
            [request.to_string()],
            |r| r.get(0),
        )
        .optional()?;
    json.map(|json| decode(&json)).transpose()
}

pub(super) fn publication(
    db: &Connection,
    request: Uuid,
) -> Result<ObservedReviewPublication, HostCatalogError> {
    // A Sending record contains a complete GitHub payload. Never load it into
    // management memory simply to display its delivery state.
    let json: Option<String> = db.query_row(
        "SELECT json_object('state',json_extract(record_json,'$.state'),
        'review_id',json_extract(record_json,'$.review_id'),'url',json_extract(record_json,'$.url'),
        'reason',json_extract(record_json,'$.reason')) FROM host_review_publications WHERE request_id=?1",
        [request.to_string()], |r| r.get(0),
    ).optional()?;
    json.map_or(Ok(ObservedReviewPublication::NotRecorded), |json| {
        decode(&json)
    })
}

pub(super) fn repositories(
    db: &Connection,
) -> Result<Vec<crate::GitHubReviewRepository>, HostCatalogError> {
    let mut query =
        db.prepare("SELECT record_json FROM host_review_repositories ORDER BY repository_id")?;
    query
        .query_map([], |r| r.get::<_, String>(0))?
        .map(|json| decode(&json?))
        .collect()
}
