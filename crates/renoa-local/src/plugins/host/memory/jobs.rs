//! The jobs of `renoa.memory`'s nightly runs. Each is frozen, its Mnem id,
//! prompt and time stored, before its command is admitted to the memory
//! session, so a retry reuses them and never asks Mnem again; and each is
//! closed by a receipt keyed by that command once its answer is applied, or
//! once it is known it will not be.
//!
//! Dream counts a declined answer, and a draft it rejects (an `Ok` outcome of
//! `Rejected`), toward giving up each time either is applied; it refuses an
//! answer with an error only for a stale job, a busy notebook, or a notebook
//! it cannot read or write, and counts none of those. So a Dream job is
//! marked before its answer is applied, and a restart that finds the mark
//! never applies it again. A job whose command cannot run is counted once per
//! run that tried it, and abandoned after `ATTEMPTS` (see `pass`).

use std::path::Path;

use jiff::civil::DateTime;
use mnem::{compactor::SummaryId, dream::DreamId, dream::Trigger, span::Span};
use renoa_kernel::AgentId;
use rusqlite::{OptionalExtension as _, TransactionBehavior, params};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::host::catalog::{self, HostCatalogError};

/// One row per job. A job without a receipt keeps its prompt, which the
/// receipt clears; the kernel admits nothing else in the session while its
/// command is unfinished, so an agent has at most one.
pub(crate) const JOBS_TABLE: &str = "CREATE TABLE IF NOT EXISTS host_memory_jobs (
        command_id TEXT PRIMARY KEY,
        agent_id TEXT NOT NULL REFERENCES host_agents(agent_id),
        run_id TEXT NOT NULL,
        position INTEGER NOT NULL CHECK (position >= 0),
        principal_id TEXT NOT NULL,
        job_json TEXT NOT NULL CHECK (json_valid(job_json)),
        prompt TEXT,
        frozen_at_ms INTEGER NOT NULL,
        local_time TEXT NOT NULL,
        applying INTEGER NOT NULL DEFAULT 0 CHECK (applying IN (0, 1)),
        attempts INTEGER NOT NULL DEFAULT 0 CHECK (attempts >= 0),
        receipt TEXT CHECK (receipt IN ('saved','settled','retried','dreamed','declined','rejected','conflict','invalid','busy','failed','unanswered','interrupted','abandoned')),
        CHECK ((prompt IS NULL) = (receipt IS NOT NULL)),
        UNIQUE (run_id, position)
    ) STRICT;
    CREATE UNIQUE INDEX IF NOT EXISTS host_memory_jobs_unsettled
        ON host_memory_jobs(agent_id) WHERE receipt IS NULL;";

/// The one definition of a job still to settle: it has no receipt yet.
const UNSETTLED: &str = "receipt IS NULL";

const COLUMNS: &str =
    "command_id,principal_id,job_json,prompt,frozen_at_ms,local_time,receipt,applying";

/// The Mnem job a command answers, as Mnem identifies it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub(super) enum JobId {
    Compaction {
        lo: u64,
        hi: u64,
        attempt: usize,
        digest: u64,
    },
    /// A nightly dream; it is the only trigger this plugin runs.
    Dream { first: u64, end: u64 },
}

impl JobId {
    pub(super) fn compaction(id: &SummaryId) -> Self {
        Self::Compaction {
            lo: id.block.lo(),
            hi: id.block.hi(),
            attempt: id.attempt,
            digest: id.digest,
        }
    }

    pub(super) const fn dream(id: &DreamId) -> Self {
        Self::Dream {
            first: id.first,
            end: id.end,
        }
    }

    pub(super) const fn kind(self) -> JobKind {
        match self {
            Self::Compaction { .. } => JobKind::Compaction,
            Self::Dream { .. } => JobKind::Dream,
        }
    }
}

/// What Mnem is asked to apply an answer to.
pub(super) enum Applying {
    Compaction(SummaryId),
    Dream(DreamId),
}

impl TryFrom<JobId> for Applying {
    type Error = HostCatalogError;

    fn try_from(id: JobId) -> Result<Self, Self::Error> {
        Ok(match id {
            JobId::Compaction {
                lo,
                hi,
                attempt,
                digest,
            } => Self::Compaction(SummaryId {
                block: Span::try_new(lo, hi).map_err(|error| {
                    HostCatalogError::Invalid(format!("stored memory job block: {error}"))
                })?,
                attempt,
                digest,
            }),
            JobId::Dream { first, end } => Self::Dream(DreamId {
                first,
                end,
                trigger: Trigger::Nightly,
            }),
        })
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum JobKind {
    Compaction,
    Dream,
}

impl JobKind {
    pub(crate) const fn as_str(self) -> &'static str {
        match self {
            Self::Compaction => "compaction",
            Self::Dream => "dream",
        }
    }
}

/// How a job settled: Mnem's answer to its apply, or why it was not applied.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Receipt {
    /// Compaction saved the summary.
    Saved,
    /// Nothing more to do: the answer was applied already, or a later job
    /// covered it.
    Settled,
    /// Compaction's line was too long; the next job asks again.
    Retried,
    /// Dream wrote a new `MIND.md`.
    Dreamed,
    /// The model declined; Dream counted it toward giving up.
    Declined,
    /// Dream refused the draft and counted it toward giving up.
    Rejected,
    /// The job was stale: the notebook moved on since it was made.
    Conflict,
    /// Compaction cannot use the answer, such as one that repeats the prompt.
    /// Dream never refuses an answer this way: it rejects a bad draft.
    Invalid,
    /// Another dream held the notebook.
    Busy,
    /// The notebook could not be read or written.
    Failed,
    /// The model call failed, so nothing was applied.
    Unanswered,
    /// A restart found this Dream answer's apply begun and its receipt
    /// unwritten, so it is not applied again.
    Interrupted,
    /// Its command could not run on `ATTEMPTS` runs, so it was closed without
    /// an answer, and the agent moved to a new memory session.
    Abandoned,
}

impl Receipt {
    pub(super) const ALL: [Self; 13] = [
        Self::Saved,
        Self::Settled,
        Self::Retried,
        Self::Dreamed,
        Self::Declined,
        Self::Rejected,
        Self::Conflict,
        Self::Invalid,
        Self::Busy,
        Self::Failed,
        Self::Unanswered,
        Self::Interrupted,
        Self::Abandoned,
    ];

    /// The stored name, the same as the serialized one.
    pub(super) const fn as_str(self) -> &'static str {
        match self {
            Self::Saved => "saved",
            Self::Settled => "settled",
            Self::Retried => "retried",
            Self::Dreamed => "dreamed",
            Self::Declined => "declined",
            Self::Rejected => "rejected",
            Self::Conflict => "conflict",
            Self::Invalid => "invalid",
            Self::Busy => "busy",
            Self::Failed => "failed",
            Self::Unanswered => "unanswered",
            Self::Interrupted => "interrupted",
            Self::Abandoned => "abandoned",
        }
    }

    fn parse(stored: &str) -> Result<Self, HostCatalogError> {
        Self::ALL
            .into_iter()
            .find(|receipt| receipt.as_str() == stored)
            .ok_or_else(|| {
                HostCatalogError::Invalid(format!("unknown memory job receipt `{stored}`"))
            })
    }
}

/// What a job freezes: whose notebook, Mnem's id and prompt, and the time
/// it was made, as an instant and as the notebook's wall time then.
pub(super) struct Job {
    pub(super) principal: Uuid,
    pub(super) id: JobId,
    pub(super) prompt: String,
    pub(super) frozen_at_ms: i64,
    pub(super) local_time: DateTime,
}

/// A frozen job whose command may be unadmitted, unfinished or settled, and
/// whose answer is not yet known to be applied.
pub(super) struct Pending {
    pub(super) command: Uuid,
    pub(super) job: Job,
    /// Its answer's apply began; set only for a Dream job.
    pub(super) applying: bool,
}

/// One job of a run, as far as it got.
pub(super) struct Done {
    pub(super) principal: Uuid,
    pub(super) kind: JobKind,
    /// `None` only for a job a crash left unsettled.
    pub(super) receipt: Option<Receipt>,
}

pub(crate) fn initialize(transaction: &rusqlite::Transaction<'_>) -> rusqlite::Result<()> {
    transaction.execute_batch(JOBS_TABLE)
}

/// Freezes `job` as the next job of `run`. Settled jobs of the agent's
/// earlier runs go in the same transaction, so an agent keeps the receipts
/// of its latest run alone.
pub(super) fn freeze(
    database: &Path,
    agent: AgentId,
    run: Uuid,
    job: Job,
) -> Result<Pending, HostCatalogError> {
    let mut connection = catalog::open_verified(database)?;
    let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let position: i64 = transaction.query_row(
        "SELECT count(*) FROM host_memory_jobs WHERE run_id = ?1",
        [run.to_string()],
        |row| row.get(0),
    )?;
    let command = crate::stable_id::stable_id(&format!("renoa.memory.job.v1:{run}:{position}"));
    transaction.execute(
        &format!(
            "DELETE FROM host_memory_jobs WHERE agent_id = ?1 AND run_id <> ?2 AND NOT ({UNSETTLED})"
        ),
        params![agent.to_string(), run.to_string()],
    )?;
    transaction.execute(
        "INSERT INTO host_memory_jobs(command_id, agent_id, run_id, position, principal_id,
            job_json, prompt, frozen_at_ms, local_time)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
        params![
            command.to_string(),
            agent.to_string(),
            run.to_string(),
            position,
            job.principal.to_string(),
            serde_json::to_string(&job.id).map_err(|error| invalid(&error))?,
            job.prompt,
            job.frozen_at_ms,
            job.local_time.to_string(),
        ],
    )?;
    transaction.commit()?;
    Ok(Pending {
        command,
        job,
        applying: false,
    })
}

/// The agent's job still to settle, from any run.
pub(super) fn unsettled(
    database: &Path,
    agent: AgentId,
) -> Result<Option<Pending>, HostCatalogError> {
    let connection = catalog::open_verified(database)?;
    let row = connection
        .query_row(
            &format!("SELECT {COLUMNS} FROM host_memory_jobs WHERE agent_id = ?1 AND {UNSETTLED}"),
            [agent.to_string()],
            stored,
        )
        .optional()?;
    row.map(|mut row| {
        let prompt = row.prompt.take().ok_or_else(|| {
            HostCatalogError::Invalid("an unsettled memory job has no prompt".to_owned())
        })?;
        Ok(Pending {
            command: parse_id(&row.command)?,
            job: Job {
                principal: parse_id(&row.principal)?,
                id: row.id()?,
                prompt,
                frozen_at_ms: row.frozen_at_ms,
                local_time: row.local_time.parse().map_err(|error: jiff::Error| {
                    HostCatalogError::Invalid(format!("stored memory job time: {error}"))
                })?,
            },
            applying: row.applying,
        })
    })
    .transpose()
}

/// The jobs of `run`, in the order they were frozen.
pub(super) fn of_run(database: &Path, run: Uuid) -> Result<Vec<Done>, HostCatalogError> {
    let connection = catalog::open_verified(database)?;
    let rows = connection
        .prepare(&format!(
            "SELECT {COLUMNS} FROM host_memory_jobs WHERE run_id = ?1 ORDER BY position"
        ))?
        .query_map([run.to_string()], stored)?
        .collect::<Result<Vec<_>, _>>()?;
    rows.into_iter()
        .map(|row| {
            Ok(Done {
                principal: parse_id(&row.principal)?,
                kind: row.id()?.kind(),
                receipt: row.receipt.as_deref().map(Receipt::parse).transpose()?,
            })
        })
        .collect()
}

/// Marks the unsettled job of `command` as having begun to apply its answer,
/// durably, before Mnem is called.
pub(super) fn mark_applying(database: &Path, command: Uuid) -> Result<(), HostCatalogError> {
    let connection = catalog::open_verified(database)?;
    let marked = connection.execute(
        &format!("UPDATE host_memory_jobs SET applying = 1 WHERE command_id = ?1 AND {UNSETTLED}"),
        [command.to_string()],
    )?;
    exactly_one(marked, command)
}

/// Counts one more run whose attempt at the unsettled job of `command` could
/// not run its command, and returns how many have.
pub(super) fn failed_attempt(database: &Path, command: Uuid) -> Result<u32, HostCatalogError> {
    let connection = catalog::open_verified(database)?;
    let attempts: Option<i64> = connection
        .query_row(
            &format!(
                "UPDATE host_memory_jobs SET attempts = attempts + 1
                 WHERE command_id = ?1 AND {UNSETTLED} RETURNING attempts"
            ),
            [command.to_string()],
            |row| row.get(0),
        )
        .optional()?;
    let attempts = attempts.ok_or_else(|| {
        HostCatalogError::Invalid(format!(
            "memory job {command} is unknown or settled already"
        ))
    })?;
    u32::try_from(attempts)
        .map_err(|error| HostCatalogError::Invalid(format!("memory job attempts: {error}")))
}

/// Closes the job of `command` with `receipt` and drops its prompt. A job
/// settles once: a second receipt is refused.
pub(super) fn settle(
    database: &Path,
    command: Uuid,
    receipt: Receipt,
) -> Result<(), HostCatalogError> {
    settle_in(&catalog::open_verified(database)?, command, receipt)
}

/// [`settle`] on `connection`, inside a caller's transaction.
pub(super) fn settle_in(
    connection: &rusqlite::Connection,
    command: Uuid,
    receipt: Receipt,
) -> Result<(), HostCatalogError> {
    let settled = connection.execute(
        &format!(
            "UPDATE host_memory_jobs SET receipt = ?2, prompt = NULL
             WHERE command_id = ?1 AND {UNSETTLED}"
        ),
        params![command.to_string(), receipt.as_str()],
    )?;
    exactly_one(settled, command)
}

fn exactly_one(changed: usize, command: Uuid) -> Result<(), HostCatalogError> {
    if changed == 1 {
        Ok(())
    } else {
        Err(HostCatalogError::Invalid(format!(
            "memory job {command} is unknown or settled already"
        )))
    }
}

struct Stored {
    command: String,
    principal: String,
    job: String,
    prompt: Option<String>,
    frozen_at_ms: i64,
    local_time: String,
    receipt: Option<String>,
    applying: bool,
}

impl Stored {
    fn id(&self) -> Result<JobId, HostCatalogError> {
        serde_json::from_str(&self.job).map_err(|error| invalid(&error))
    }
}

fn stored(row: &rusqlite::Row<'_>) -> rusqlite::Result<Stored> {
    Ok(Stored {
        command: row.get(0)?,
        principal: row.get(1)?,
        job: row.get(2)?,
        prompt: row.get(3)?,
        frozen_at_ms: row.get(4)?,
        local_time: row.get(5)?,
        receipt: row.get(6)?,
        applying: row.get(7)?,
    })
}

fn parse_id(value: &str) -> Result<Uuid, HostCatalogError> {
    Uuid::parse_str(value)
        .map_err(|error| HostCatalogError::Invalid(format!("stored memory job id: {error}")))
}

fn invalid(error: &serde_json::Error) -> HostCatalogError {
    HostCatalogError::Invalid(format!("memory job: {error}"))
}

#[cfg(test)]
mod tests {
    use super::{JOBS_TABLE, Receipt};

    /// The table's `CHECK` admits every receipt the code writes and nothing
    /// else, so the two lists cannot drift apart.
    #[test]
    fn the_receipt_check_admits_exactly_every_receipt() {
        let connection = rusqlite::Connection::open_in_memory().expect("catalog");
        connection
            .execute_batch(
                "CREATE TABLE host_agents (agent_id TEXT PRIMARY KEY) STRICT;
                 INSERT INTO host_agents VALUES ('agent');",
            )
            .expect("agents");
        connection.execute_batch(JOBS_TABLE).expect("jobs");
        let insert = |position: usize, receipt: &str| {
            connection.execute(
                "INSERT INTO host_memory_jobs(command_id, agent_id, run_id, position, principal_id,
                    job_json, prompt, frozen_at_ms, local_time, receipt)
                 VALUES (?1, 'agent', 'run', ?2, 'person', '{}', NULL, 0, '2026-11-03T04:00', ?3)",
                rusqlite::params![
                    position.to_string(),
                    i64::try_from(position).expect("a small position"),
                    receipt
                ],
            )
        };
        for (position, receipt) in Receipt::ALL.into_iter().enumerate() {
            insert(position, receipt.as_str())
                .unwrap_or_else(|error| panic!("{}: {error}", receipt.as_str()));
            assert_eq!(Receipt::parse(receipt.as_str()).ok(), Some(receipt));
        }
        assert!(insert(Receipt::ALL.len(), "applied").is_err());
    }
}
