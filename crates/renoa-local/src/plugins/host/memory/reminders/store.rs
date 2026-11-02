//! What the catalog keeps of `renoa.memory`'s reminders: which agenda item
//! each trigger hands over and on which attempt, and how its run settled.

use std::collections::HashMap;

use jiff::civil::DateTime;
use renoa_kernel::AgentId;
use rusqlite::{Connection, OptionalExtension as _, params};
use uuid::Uuid;

use super::{Item, Reminder};
use crate::{RunStatus, plugins::PluginError};

/// One row per reminder trigger. `attempt` is 0, or 1 for the one retry
/// after a failed run. `outcome` is set once the run settled: `fired` once
/// the item was recorded fired, `failed` once a failed run was seen. A row
/// goes with its trigger.
pub(crate) const TABLE: &str = "CREATE TABLE IF NOT EXISTS host_memory_reminders (
        automation_id TEXT PRIMARY KEY
            REFERENCES host_plugin_triggers(automation_id) ON DELETE CASCADE,
        principal_id TEXT NOT NULL,
        item_id TEXT NOT NULL,
        due TEXT NOT NULL,
        attempt INTEGER NOT NULL CHECK (attempt IN (0, 1)),
        outcome TEXT CHECK (outcome IN ('fired', 'failed'))
    ) STRICT;";

/// How a reminder's run settled.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Outcome {
    /// It succeeded and the item was recorded fired.
    Fired,
    /// It failed; the item stays unfired.
    Failed,
}

impl Outcome {
    const fn as_str(self) -> &'static str {
        match self {
            Self::Fired => "fired",
            Self::Failed => "failed",
        }
    }
}

/// What the scheduler did with one reminder trigger.
pub(super) struct State {
    /// Whether it has a run, finished or not.
    pub(super) ran: bool,
    /// Its run's status, once finished.
    pub(super) status: Option<RunStatus>,
    /// When its run finished, or else when it was admitted.
    pub(super) ended_at_ms: Option<i64>,
    pub(super) deleted: bool,
}

/// A finished run whose reminder has not settled.
pub(super) struct Unsettled {
    pub(super) automation: Uuid,
    pub(super) reminder: Reminder,
    pub(super) status: RunStatus,
    /// When it finished, or else when it was admitted.
    pub(super) at_ms: i64,
}

pub(crate) fn initialize(transaction: &rusqlite::Transaction<'_>) -> rusqlite::Result<()> {
    transaction.execute_batch(TABLE)
}

/// Records that the trigger `automation`, being created, is `reminder`.
pub(super) fn own(
    transaction: &rusqlite::Transaction<'_>,
    automation: Uuid,
    reminder: &Reminder,
) -> rusqlite::Result<()> {
    transaction
        .execute(
            "INSERT INTO host_memory_reminders(automation_id, principal_id, item_id, due, attempt)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![
                automation.to_string(),
                reminder.principal.to_string(),
                reminder.item,
                mnem::fired::minutes(reminder.due),
                reminder.attempt,
            ],
        )
        .map(drop)
}

/// Every reminder trigger of `agent`, by id, with what became of it.
pub(super) fn states(db: &Connection, agent: AgentId) -> Result<HashMap<Uuid, State>, PluginError> {
    db.prepare(
        "SELECT m.automation_id, r.id IS NOT NULL, r.status,
             coalesce(r.finished_at_ms, r.admitted_at_ms),
             EXISTS(SELECT 1 FROM host_automation_deletions d WHERE d.automation_id = m.automation_id)
         FROM host_memory_reminders m
         JOIN host_automations a ON a.id = m.automation_id
         LEFT JOIN host_automation_runs r ON r.automation_id = m.automation_id
         WHERE a.agent_id = ?1",
    )?
    .query_map([agent.to_string()], |row| {
        Ok((
            row.get::<_, String>(0)?,
            State {
                ran: row.get(1)?,
                status: row.get(2)?,
                ended_at_ms: row.get(3)?,
                deleted: row.get(4)?,
            },
        ))
    })?
    .map(|row| {
        let (automation, state) = row?;
        Ok((parse(&automation)?, state))
    })
    .collect()
}

/// For each person, how many of their reminders' runs admitted at or after
/// `since_ms` started more than `late_ms` after their due time and were not
/// skipped.
pub(super) fn late_runs(
    db: &Connection,
    since_ms: i64,
    late_ms: i64,
) -> Result<HashMap<Uuid, usize>, PluginError> {
    db.prepare(
        "SELECT m.principal_id, count(*) FROM host_memory_reminders m
         JOIN host_automation_runs r ON r.automation_id = m.automation_id
         WHERE r.admitted_at_ms >= ?1 AND r.admitted_at_ms - r.due_ms > ?2
           AND (r.status IS NULL OR r.status != ?3)
         GROUP BY m.principal_id",
    )?
    .query_map(
        params![since_ms, late_ms, RunStatus::Skipped.as_str()],
        |row| Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?)),
    )?
    .map(|row| {
        let (principal, count) = row?;
        Ok((
            parse(&principal)?,
            usize::try_from(count)
                .map_err(|error| PluginError::Invalid(format!("a late reminder count: {error}")))?,
        ))
    })
    .collect()
}

/// Every reminder whose run finished, not skipped, and has not settled.
pub(super) fn unsettled(db: &Connection) -> Result<Vec<Unsettled>, PluginError> {
    db.prepare(
        "SELECT m.automation_id, m.principal_id, m.item_id, m.due, m.attempt, r.status,
             coalesce(r.finished_at_ms, r.admitted_at_ms)
         FROM host_memory_reminders m
         JOIN host_automation_runs r ON r.automation_id = m.automation_id
         WHERE m.outcome IS NULL AND r.output IS NOT NULL AND r.status != ?1",
    )?
    .query_map([RunStatus::Skipped.as_str()], |row| {
        Ok((
            (
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, u8>(4)?,
            ),
            row.get::<_, RunStatus>(5)?,
            row.get::<_, i64>(6)?,
        ))
    })?
    .map(|row| {
        let ((automation, principal, item, due, attempt), status, at_ms) = row?;
        Ok(Unsettled {
            automation: parse(&automation)?,
            reminder: reminder(&principal, item, &due, attempt)?,
            status,
            at_ms,
        })
    })
    .collect()
}

/// Every armed reminder trigger due at `now_ms` that has no run yet.
pub(super) fn due(db: &Connection, now_ms: i64) -> Result<Vec<(Uuid, Reminder)>, PluginError> {
    db.prepare(
        "SELECT m.automation_id, m.principal_id, m.item_id, m.due, m.attempt
         FROM host_memory_reminders m JOIN host_automations a ON a.id = m.automation_id
         WHERE a.enabled = 1 AND a.next_due_ms <= ?1
           AND NOT EXISTS(SELECT 1 FROM host_automation_deletions d WHERE d.automation_id = a.id)
           AND NOT EXISTS(SELECT 1 FROM host_automation_runs r WHERE r.automation_id = a.id)",
    )?
    .query_map([now_ms], |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, String>(2)?,
            row.get::<_, String>(3)?,
            row.get::<_, u8>(4)?,
        ))
    })?
    .map(|row| {
        let (automation, principal, item, due, attempt) = row?;
        Ok((
            parse(&automation)?,
            reminder(&principal, item, &due, attempt)?,
        ))
    })
    .collect()
}

/// Whether `principal`'s `item` due at `due` has a reminder, on any
/// attempt.
pub(super) fn known(
    db: &Connection,
    principal: Uuid,
    item: &str,
    due: DateTime,
) -> Result<bool, PluginError> {
    Ok(db.query_row(
        "SELECT EXISTS(SELECT 1 FROM host_memory_reminders
             WHERE principal_id = ?1 AND item_id = ?2 AND due = ?3)",
        params![principal.to_string(), item, mnem::fired::minutes(due)],
        |row| row.get(0),
    )?)
}

/// Records how `automation`'s run settled, once.
pub(super) fn settle(
    db: &Connection,
    automation: Uuid,
    outcome: Outcome,
) -> Result<(), PluginError> {
    db.execute(
        "UPDATE host_memory_reminders SET outcome = ?2
         WHERE automation_id = ?1 AND outcome IS NULL",
        params![automation.to_string(), outcome.as_str()],
    )?;
    Ok(())
}

/// The reminder `automation` is, if it is one.
pub(super) fn stored(db: &Connection, automation: Uuid) -> Result<Option<Reminder>, PluginError> {
    db.query_row(
        "SELECT principal_id, item_id, due, attempt FROM host_memory_reminders
         WHERE automation_id = ?1",
        [automation.to_string()],
        |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, u8>(3)?,
            ))
        },
    )
    .optional()?
    .map(|(principal, item, due, attempt)| reminder(&principal, item, &due, attempt))
    .transpose()
}

/// The items of `principal`'s reminders that `agent` has, as stored: what
/// a notebook that cannot be read keeps. A quiet item is never stored.
pub(super) fn kept(
    db: &Connection,
    agent: AgentId,
    principal: Uuid,
) -> Result<Vec<Item>, PluginError> {
    db.prepare(
        "SELECT DISTINCT m.item_id, m.due FROM host_memory_reminders m
         JOIN host_automations a ON a.id = m.automation_id
         WHERE m.principal_id = ?1 AND a.agent_id = ?2 ORDER BY m.due, m.item_id",
    )?
    .query_map(params![principal.to_string(), agent.to_string()], |row| {
        Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
    })?
    .map(|row| {
        let (id, due) = row?;
        Ok(Item {
            id,
            due: parse_due(&due)?,
            notify: true,
        })
    })
    .collect()
}

/// Whether the command `request` is a reminder's run.
pub(crate) fn is_run(db: &Connection, request: Uuid) -> Result<bool, PluginError> {
    Ok(db.query_row(
        "SELECT EXISTS(SELECT 1 FROM host_automation_runs r
             JOIN host_memory_reminders m ON m.automation_id = r.automation_id
             WHERE r.id = ?1)",
        [request.to_string()],
        |row| row.get(0),
    )?)
}

fn reminder(
    principal: &str,
    item: String,
    due: &str,
    attempt: u8,
) -> Result<Reminder, PluginError> {
    Ok(Reminder {
        principal: parse(principal)?,
        item,
        due: parse_due(due)?,
        attempt,
    })
}

fn parse(value: &str) -> Result<Uuid, PluginError> {
    Uuid::parse_str(value)
        .map_err(|error| PluginError::Invalid(format!("a stored reminder id: {error}")))
}

fn parse_due(value: &str) -> Result<DateTime, PluginError> {
    value
        .parse()
        .map_err(|error| PluginError::Invalid(format!("a reminder's due time: {error}")))
}
