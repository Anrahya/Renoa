use std::path::Path;

use rusqlite::{Connection, OptionalExtension as _, Transaction, TransactionBehavior, params};
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use super::{
    GitHubReviewAdmission, GitHubReviewCommand, GitHubReviewError, GitHubReviewPolicy,
    GitHubReviewReply, GitHubReviewRepository, GitHubReviewRequest, GitHubReviewSkip, active,
    catalog, validate_target,
    webhook::{Delivery, Event, PullRequestEvent},
};

pub(in crate::host) fn initialize(tx: &Transaction<'_>) -> Result<(), catalog::HostCatalogError> {
    tx.execute_batch(
        "CREATE TABLE IF NOT EXISTS host_review_repositories (
        repository_id INTEGER PRIMARY KEY CHECK(repository_id>0),
        agent_id TEXT NOT NULL REFERENCES host_agents(agent_id),
        record_json TEXT NOT NULL CHECK(json_valid(record_json))
    ) STRICT;
    CREATE TABLE IF NOT EXISTS host_review_operations (
        operation_id TEXT PRIMARY KEY, request_json TEXT NOT NULL, result_json TEXT NOT NULL
    ) STRICT;
    CREATE TABLE IF NOT EXISTS host_review_requests (
        sequence INTEGER PRIMARY KEY AUTOINCREMENT, id TEXT NOT NULL UNIQUE,
        automatic_key TEXT UNIQUE,
        repository_id INTEGER NOT NULL REFERENCES host_review_repositories(repository_id),
        repository_json TEXT NOT NULL CHECK(json_valid(repository_json)),
        pull_number INTEGER NOT NULL CHECK(pull_number>0), base_sha TEXT NOT NULL,
        head_sha TEXT NOT NULL, admitted_at_ms INTEGER NOT NULL CHECK(admitted_at_ms>=0)
    ) STRICT;
    CREATE TABLE IF NOT EXISTS host_review_deliveries (
        delivery_id TEXT PRIMARY KEY, digest BLOB NOT NULL, result_json TEXT NOT NULL
    ) STRICT;",
    )?;
    super::runs::initialize(tx)?;
    super::jobs::initialize(tx)?;
    Ok(())
}

pub(super) fn manage(
    path: &Path,
    command: &GitHubReviewCommand,
    now_ms: i64,
    cancellation: &CancellationToken,
) -> Result<GitHubReviewReply, GitHubReviewError> {
    active(cancellation)?;
    let mut db = catalog::open_verified(path)?;
    if let Some(reply) = read_command(&db, path, command)? {
        return Ok(reply);
    }
    let (GitHubReviewCommand::SetRepository { operation_id, .. }
    | GitHubReviewCommand::Request { operation_id, .. }) = command
    else {
        return Err(GitHubReviewError::Invalid(
            "listing is not a mutation".to_owned(),
        ));
    };
    if operation_id.is_nil() || now_ms < 0 {
        return Err(GitHubReviewError::Invalid(
            "invalid operation ID or admission time".to_owned(),
        ));
    }
    let tx = db.transaction_with_behavior(TransactionBehavior::Immediate)?;
    active(cancellation)?;
    let request = serde_json::to_string(command)?;
    let receipt: Option<(String, String)> = tx
        .query_row(
            "SELECT request_json,result_json FROM host_review_operations WHERE operation_id=?1",
            [operation_id.to_string()],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()?;
    if let Some((prior, result)) = receipt {
        if prior != request {
            return Err(GitHubReviewError::Conflict);
        }
        return Ok(serde_json::from_str(&result)?);
    }
    let result = match command {
        GitHubReviewCommand::SetRepository {
            expected_revision,
            policy,
            ..
        } => GitHubReviewReply::Repository {
            record: set_repository(&tx, *expected_revision, policy)?,
        },
        GitHubReviewCommand::Request {
            repository_id,
            pull_number,
            reported_base_sha,
            reported_head_sha,
            ..
        } => {
            validate_target(*pull_number, reported_base_sha, reported_head_sha)?;
            let repository = repository(&tx, *repository_id)?.ok_or(GitHubReviewError::NotFound)?;
            insert_request(
                &tx,
                *operation_id,
                None,
                &repository,
                &RequestedPull {
                    number: *pull_number,
                    base: reported_base_sha,
                    head: reported_head_sha,
                    now_ms,
                },
            )?;
            GitHubReviewReply::Request {
                record: get_request(&tx, *operation_id)?,
            }
        }
        GitHubReviewCommand::Repositories { .. }
        | GitHubReviewCommand::Requests { .. }
        | GitHubReviewCommand::Publication { .. }
        | GitHubReviewCommand::Run { .. } => {
            return Err(GitHubReviewError::Invalid(
                "listing is not a mutation".to_owned(),
            ));
        }
    };
    active(cancellation)?;
    tx.execute(
        "INSERT INTO host_review_operations VALUES(?1,?2,?3)",
        params![
            operation_id.to_string(),
            request,
            serde_json::to_string(&result)?
        ],
    )?;
    tx.commit()?;
    Ok(result)
}

fn read_command(
    db: &Connection,
    path: &Path,
    command: &GitHubReviewCommand,
) -> Result<Option<GitHubReviewReply>, GitHubReviewError> {
    Ok(Some(match command {
        GitHubReviewCommand::Publication { request_id } => GitHubReviewReply::Publication {
            record: super::publication::get(path, *request_id)?,
        },
        GitHubReviewCommand::Run { request_id } => GitHubReviewReply::Run {
            record: super::runs::get(db, *request_id)?,
        },
        GitHubReviewCommand::Repositories { after } => {
            let mut q = db.prepare("SELECT record_json FROM host_review_repositories WHERE repository_id>?1 ORDER BY repository_id LIMIT 20")?;
            GitHubReviewReply::Repositories {
                records: q
                    .query_map([after.unwrap_or(0)], |row| json(row, 0))?
                    .collect::<Result<Vec<_>, _>>()?,
            }
        }
        GitHubReviewCommand::Requests { after } => {
            let mut q = db.prepare("SELECT sequence,id,repository_json,pull_number,base_sha,head_sha,admitted_at_ms FROM host_review_requests WHERE sequence>?1 ORDER BY sequence LIMIT 20")?;
            GitHubReviewReply::Requests {
                records: q
                    .query_map([after], request_row)?
                    .collect::<Result<Vec<_>, _>>()?,
            }
        }
        GitHubReviewCommand::SetRepository { .. } | GitHubReviewCommand::Request { .. } => {
            return Ok(None);
        }
    }))
}

fn set_repository(
    tx: &Transaction<'_>,
    expected: Option<i64>,
    policy: &GitHubReviewPolicy,
) -> Result<GitHubReviewRepository, GitHubReviewError> {
    let parts: Vec<_> = policy.full_name.split('/').collect();
    if policy.repository_id <= 0
        || policy.installation_id <= 0
        || parts.len() != 2
        || parts.iter().any(|part| {
            part.is_empty()
                || part.len() > 100
                || matches!(*part, "." | "..")
                || !part
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b"-_.".contains(&b))
        })
    {
        return Err(GitHubReviewError::Invalid(
            "provide positive repository/installation IDs and owner/repository".to_owned(),
        ));
    }
    let known: bool = tx.query_row(
        "SELECT EXISTS(SELECT 1 FROM host_bots WHERE agent_id=?1)",
        [policy.agent_id.to_string()],
        |row| row.get(0),
    )?;
    if !known {
        return Err(GitHubReviewError::Invalid(
            "reviewer must be a specialist owned by this Host".to_owned(),
        ));
    }
    let previous = repository(tx, policy.repository_id)?;
    if previous.as_ref().map(|record| record.revision) != expected {
        return Err(GitHubReviewError::Conflict);
    }
    let revision = expected
        .unwrap_or(0)
        .checked_add(1)
        .ok_or(GitHubReviewError::Conflict)?;
    let record = GitHubReviewRepository {
        revision,
        policy: policy.clone(),
    };
    tx.execute("INSERT INTO host_review_repositories VALUES(?1,?2,?3) ON CONFLICT(repository_id) DO UPDATE SET agent_id=excluded.agent_id,record_json=excluded.record_json",
        params![policy.repository_id, policy.agent_id.to_string(), serde_json::to_string(&record)?])?;
    Ok(record)
}

pub(super) fn admit(
    path: &Path,
    delivery: &Delivery,
    now_ms: i64,
    cancellation: &CancellationToken,
) -> Result<GitHubReviewAdmission, GitHubReviewError> {
    active(cancellation)?;
    if now_ms < 0 {
        return Err(GitHubReviewError::Invalid(
            "negative admission time".to_owned(),
        ));
    }
    let mut db = catalog::open_verified(path)?;
    let tx = db.transaction_with_behavior(TransactionBehavior::Immediate)?;
    active(cancellation)?;
    let receipt: Option<(Vec<u8>, String)> = tx
        .query_row(
            "SELECT digest,result_json FROM host_review_deliveries WHERE delivery_id=?1",
            [delivery.id.to_string()],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()?;
    if let Some((digest, result)) = receipt {
        if digest != delivery.digest {
            return Err(GitHubReviewError::Conflict);
        }
        return Ok(serde_json::from_str(&result)?);
    }
    let result = match &delivery.event {
        Event::Ignored(reason) => GitHubReviewAdmission::Ignored { reason: *reason },
        Event::PullRequest(event) => admit_pull(&tx, event, now_ms)?,
    };
    active(cancellation)?;
    tx.execute(
        "INSERT INTO host_review_deliveries VALUES(?1,?2,?3)",
        params![
            delivery.id.to_string(),
            delivery.digest,
            serde_json::to_string(&result)?
        ],
    )?;
    tx.commit()?;
    Ok(result)
}

fn admit_pull(
    tx: &Transaction<'_>,
    event: &PullRequestEvent,
    now_ms: i64,
) -> Result<GitHubReviewAdmission, GitHubReviewError> {
    let Some(repository) = repository(tx, event.repository.id)? else {
        return Ok(GitHubReviewAdmission::Ignored {
            reason: GitHubReviewSkip::UnconfiguredRepository,
        });
    };
    let policy = &repository.policy;
    if policy.installation_id != event.installation.id {
        return Err(GitHubReviewError::Authentication);
    }
    let skip = if !policy.enabled {
        Some(GitHubReviewSkip::Disabled)
    } else if event.pull_request.state == "closed" {
        Some(GitHubReviewSkip::Closed)
    } else if policy.skip_drafts && event.pull_request.draft {
        Some(GitHubReviewSkip::Draft)
    } else if !policy.triggers.contains(&event.action) {
        Some(GitHubReviewSkip::TriggerDisabled)
    } else {
        None
    };
    if let Some(reason) = skip {
        return Ok(GitHubReviewAdmission::Ignored { reason });
    }
    // Different delivery IDs/actions for this same policy and reported revision
    // request one review. Commit order is intentionally not inferred here.
    let key = serde_json::to_string(&(
        policy.repository_id,
        repository.revision,
        event.pull_request.number,
        &event.pull_request.base.sha,
        &event.pull_request.head.sha,
    ))?;
    let existing: Option<String> = tx
        .query_row(
            "SELECT id FROM host_review_requests WHERE automatic_key=?1",
            [&key],
            |row| row.get(0),
        )
        .optional()?;
    let id = if let Some(id) = existing {
        Uuid::try_parse(&id)
            .map_err(|_| GitHubReviewError::Invalid("corrupt review request ID".to_owned()))?
    } else {
        let id = Uuid::new_v4();
        insert_request(
            tx,
            id,
            Some(&key),
            &repository,
            &RequestedPull {
                number: event.pull_request.number,
                base: &event.pull_request.base.sha,
                head: &event.pull_request.head.sha,
                now_ms,
            },
        )?;
        id
    };
    Ok(GitHubReviewAdmission::Queued { request_id: id })
}

struct RequestedPull<'a> {
    number: i64,
    base: &'a str,
    head: &'a str,
    now_ms: i64,
}

fn insert_request(
    tx: &Transaction<'_>,
    id: Uuid,
    key: Option<&str>,
    repository: &GitHubReviewRepository,
    pull: &RequestedPull<'_>,
) -> Result<(), GitHubReviewError> {
    let pending: i64 = tx.query_row("SELECT count(*) FROM host_review_requests r WHERE NOT EXISTS(SELECT 1 FROM host_review_runs x WHERE x.request_id=r.id AND x.terminal=1)", [], |row| {
        row.get(0)
    })?;
    if pending >= 1024 {
        return Err(GitHubReviewError::Capacity);
    }
    tx.execute("INSERT INTO host_review_requests(id,automatic_key,repository_id,repository_json,pull_number,base_sha,head_sha,admitted_at_ms) VALUES(?1,?2,?3,?4,?5,?6,?7,?8)",
        params![id.to_string(), key, repository.policy.repository_id, serde_json::to_string(repository)?, pull.number, pull.base, pull.head, pull.now_ms])?;
    Ok(())
}

pub(super) fn repository(
    db: &Connection,
    id: i64,
) -> Result<Option<GitHubReviewRepository>, GitHubReviewError> {
    Ok(db
        .query_row(
            "SELECT record_json FROM host_review_repositories WHERE repository_id=?1",
            [id],
            |row| json(row, 0),
        )
        .optional()?)
}

pub(super) fn json<T: serde::de::DeserializeOwned>(
    row: &rusqlite::Row<'_>,
    index: usize,
) -> rusqlite::Result<T> {
    let text: String = row.get(index)?;
    serde_json::from_str(&text).map_err(|e| {
        rusqlite::Error::FromSqlConversionFailure(index, rusqlite::types::Type::Text, Box::new(e))
    })
}

fn request_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<GitHubReviewRequest> {
    let id: String = row.get(1)?;
    Ok(GitHubReviewRequest {
        sequence: row.get(0)?,
        id: Uuid::try_parse(&id).map_err(|e| {
            rusqlite::Error::FromSqlConversionFailure(1, rusqlite::types::Type::Text, Box::new(e))
        })?,
        repository: json(row, 2)?,
        pull_number: row.get(3)?,
        reported_base_sha: row.get(4)?,
        reported_head_sha: row.get(5)?,
        admitted_at_ms: row.get(6)?,
    })
}

pub(super) fn get_request(
    db: &Connection,
    id: Uuid,
) -> Result<GitHubReviewRequest, GitHubReviewError> {
    Ok(db.query_row("SELECT sequence,id,repository_json,pull_number,base_sha,head_sha,admitted_at_ms FROM host_review_requests WHERE id=?1", [id.to_string()], request_row)?)
}
