//! `renoa.memory`'s reminders: each timed item on a person's agenda, a nudge
//! in `MIND.md` or an `[at …]` in a note, becomes a one-time automation of
//! the main agent at its due time.
//!
//! Mnem decides what is on the agenda (`Memory::agenda`), whether an item is
//! still to fire when its time comes and the one message its run sends
//! (`Memory::nudge_run`), and what fired (`Memory::record_fired`). The Host
//! decides when and where each runs:
//!
//! 1. The scheduler converges the reminders before it admits a due run (see
//!    `triggers`): one trigger per person, item and due time, each with the
//!    id derived from them, for the people the main agent has conversations
//!    with. A replay converges, and an item Mnem no longer lists is no longer
//!    wanted. A person's timed items are read again only when the
//!    notebook's `AgendaStamp` changes.
//! 2. Its run is delivered to the main agent, as a run of an automation it
//!    created is, with Mnem's message as the whole submission. One Mnem says
//!    is no longer due is skipped. An item to tell the person about runs in
//!    the main agent's latest conversation with them; a quiet one runs in a
//!    conversation of the automation's own, whose reply the person is not
//!    shown.
//! 3. Once its run is recorded, the next convergence records the item fired,
//!    and only then marks it so; Mnem records an item and due time once, so a
//!    crash between the two repeats nothing. A trigger that ran is not wanted
//!    again, so its run is never repeated.
//! 4. A trigger no longer wanted is deleted once no run of it is left, and
//!    removed once its conversation is gone (see `retention`).

use std::{collections::HashMap, path::Path, sync::Mutex};

use jiff::{Timestamp, civil::DateTime};
use mnem::{heartbeat::NudgeInput, memory::AgendaStamp, mind::When};
use renoa_kernel::AgentId;
use rusqlite::{Connection, OptionalExtension as _, params};
use serde_json::json;
use uuid::Uuid;

use super::{
    NOTE, for_jobs, local_time, nightly::driver, note_pass, notebook_zone, pass::event, read_only,
};
use crate::{
    AutomationSchedule, AutomationSpec, RunStatus,
    plugins::{
        PluginError,
        host::{HostPluginId, triggers},
    },
};

const NAME: &str = "renoa.memory reminder";
const TASK: &str =
    "When an item on the person's agenda is due, hand it to the main agent in Mnem's words.";

/// Why a reminder's run is skipped.
const OFF: &str = "renoa.memory is off for this agent, or it is no longer the Host's main agent.";
const GONE: &str =
    "No longer due: it fired, went stale, or a later dream or note dropped or moved it.";
const NO_CONVERSATION: &str =
    "The main agent no longer has a conversation with this person to tell them in.";

/// One row per reminder trigger: whose agenda item it is, and whether the
/// item was recorded fired after its run. It goes with its trigger.
pub(crate) const TABLE: &str = "CREATE TABLE IF NOT EXISTS host_memory_reminders (
        automation_id TEXT PRIMARY KEY
            REFERENCES host_plugin_triggers(automation_id) ON DELETE CASCADE,
        principal_id TEXT NOT NULL,
        item_id TEXT NOT NULL,
        due TEXT NOT NULL,
        fired INTEGER NOT NULL CHECK (fired IN (0, 1))
    ) STRICT;";

pub(crate) fn initialize(transaction: &rusqlite::Transaction<'_>) -> rusqlite::Result<()> {
    transaction.execute_batch(TABLE)
}

/// One item of one person's agenda, due at a wall time in the notebook zone.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Reminder {
    principal: Uuid,
    item: String,
    due: DateTime,
}

/// How a reminder's run is delivered, decided when it is admitted.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Delivery {
    /// Mnem's `message`, in the conversation `origin`, or else in one of the
    /// automation's own.
    Send {
        message: String,
        origin: Option<Uuid>,
    },
    /// Not run, for this reason.
    Skip(&'static str),
}

/// Each timed item of an agenda: its id and when it is due.
type Timed = Vec<(String, DateTime)>;

/// Each person's timed items as of their notebook's [`AgendaStamp`], read
/// again only when it changes. It lives as long as the schedule's owner, so
/// a restart reads each agenda once more.
#[derive(Default)]
pub(crate) struct Agendas(Mutex<HashMap<Uuid, (AgendaStamp, Timed)>>);

impl Agendas {
    /// The timed items of `principal`'s agenda, overdue ones included, as
    /// Mnem lists them at `now`; none without a notebook.
    fn timed(
        &self,
        data_directory: &Path,
        principal: Uuid,
        now: DateTime,
    ) -> Result<Timed, PluginError> {
        let Some(memory) = read_only(data_directory, principal).map_err(unreadable)? else {
            return Ok(Vec::new());
        };
        // Read before the agenda, so a change between the two reads it again.
        let stamp = memory.agenda_stamp().map_err(unreadable)?;
        let mut people = self
            .0
            .lock()
            .map_err(|_| PluginError::Unavailable("the reminders' agendas lock".to_owned()))?;
        if let Some((held, items)) = people.get(&principal)
            && *held == stamp
        {
            return Ok(items.clone());
        }
        let agenda = memory.agenda(now).map_err(unreadable)?;
        if !agenda.problems.is_empty() {
            event(
                "warn",
                "memory_agenda_problems",
                &json!({"principal_id": principal, "problems": agenda.problems.len()}),
            );
        }
        let items: Vec<_> = agenda
            .timed
            .into_iter()
            .filter_map(|item| match item.when {
                When::At(due) => Some((item.id, due)),
                When::NextSession | When::Event(_) => None,
            })
            .collect();
        people.insert(principal, (stamp, items.clone()));
        Ok(items)
    }
}

/// The reminders the main agent keeps as of `now_ms`. First each item whose
/// run finished is recorded fired, so it leaves its agenda; then one trigger
/// is wanted per timed item still listed whose trigger neither ran nor was
/// deleted, for each person the main agent has a conversation with. A
/// notebook that cannot be read is logged and keeps the reminders it has, so
/// it holds up no one else's.
///
/// # Errors
///
/// Returns catalog and time failures; the scheduler keeps every trigger as
/// it was and asks again.
pub(crate) fn wanted(
    db: &Connection,
    data_directory: &Path,
    agendas: &Agendas,
    now_ms: i64,
) -> Result<Vec<(Uuid, AutomationSpec, Reminder)>, PluginError> {
    let zone = notebook_zone(db)?;
    record_fired(db, data_directory, &zone)?;
    let Some(agent) = driver(db)? else {
        return Ok(Vec::new());
    };
    let now = local_time(&zone, instant(now_ms)?);
    let mut wanted = Vec::new();
    for (principal, _) in note_pass::latest_conversations(db, agent)? {
        let items = match agendas.timed(data_directory, principal, now) {
            Ok(items) => items,
            Err(error) => {
                unread(principal, &error);
                kept(db, agent, principal)?
            }
        };
        for (item, due) in items {
            let id = triggers::automation_id_named(
                HostPluginId::Memory,
                agent,
                &format!("reminder:{principal}:{item}@{}", mnem::fired::minutes(due)),
            );
            if ran_or_deleted(db, id)? {
                continue;
            }
            let at = due
                .to_zoned(zone.clone())
                .map_err(|error| PluginError::Invalid(format!("a reminder's due time: {error}")))?
                .timestamp();
            let spec = AutomationSpec {
                agent_id: agent,
                name: NAME.to_owned(),
                prompt: TASK.to_owned(),
                schedule: AutomationSchedule::Once { at: at.to_string() },
                enabled: true,
            };
            wanted.push((
                id,
                spec,
                Reminder {
                    principal,
                    item,
                    due,
                },
            ));
        }
    }
    Ok(wanted)
}

/// The items of `principal`'s reminders `agent` keeps now, as stored.
fn kept(db: &Connection, agent: AgentId, principal: Uuid) -> Result<Timed, PluginError> {
    db.prepare(
        "SELECT m.item_id, m.due FROM host_memory_reminders m
         JOIN host_automations a ON a.id = m.automation_id
         WHERE m.principal_id = ?1 AND a.agent_id = ?2 ORDER BY m.due, m.item_id",
    )?
    .query_map(params![principal.to_string(), agent.to_string()], |row| {
        Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
    })?
    .map(|row| {
        let (item, due) = row?;
        Ok((item, parse_due(&due)?))
    })
    .collect()
}

/// Records that the trigger `automation`, being created, is `reminder`.
pub(crate) fn own(
    transaction: &rusqlite::Transaction<'_>,
    automation: Uuid,
    reminder: &Reminder,
) -> rusqlite::Result<()> {
    transaction
        .execute(
            "INSERT INTO host_memory_reminders(automation_id, principal_id, item_id, due, fired)
             VALUES (?1, ?2, ?3, ?4, 0)",
            params![
                automation.to_string(),
                reminder.principal.to_string(),
                reminder.item,
                mnem::fired::minutes(reminder.due),
            ],
        )
        .map(drop)
}

/// How the run of `automation` for `agent` admitted at `now_ms` is
/// delivered; `None` when it is no reminder.
///
/// # Errors
///
/// Returns catalog, notebook and time failures; the run is not admitted.
pub(crate) fn delivery(
    db: &Connection,
    data_directory: &Path,
    automation: Uuid,
    agent: AgentId,
    now_ms: i64,
) -> Result<Option<Delivery>, PluginError> {
    let Some(reminder) = stored(db, automation)? else {
        return Ok(None);
    };
    if driver(db)? != Some(agent) {
        return Ok(Some(Delivery::Skip(OFF)));
    }
    let now = local_time(&notebook_zone(db)?, instant(now_ms)?);
    let run = match read_only(data_directory, reminder.principal).map_err(unreadable)? {
        Some(memory) => memory
            .nudge_run(&NudgeInput {
                id: &reminder.item,
                due: reminder.due,
                now,
                note_tool: NOTE,
            })
            .map_err(unreadable)?,
        None => None,
    };
    let Some(run) = run else {
        return Ok(Some(Delivery::Skip(GONE)));
    };
    if !run.notify {
        return Ok(Some(Delivery::Send {
            message: run.message,
            origin: None,
        }));
    }
    let conversation = note_pass::latest_conversations(db, agent)?
        .into_iter()
        .find_map(|(principal, session)| (principal == reminder.principal).then_some(session));
    Ok(Some(conversation.map_or(
        Delivery::Skip(NO_CONVERSATION),
        |session| Delivery::Send {
            message: run.message,
            origin: Some(session),
        },
    )))
}

/// Records fired, in its person's notebook, every item whose reminder's run
/// finished and was not skipped, then marks it so. A run skipped never
/// reached the agent. A notebook that cannot be written is logged, and its
/// item is recorded at the next convergence.
fn record_fired(
    db: &Connection,
    data_directory: &Path,
    zone: &jiff::tz::TimeZone,
) -> Result<(), PluginError> {
    let finished = db
        .prepare(
            "SELECT m.automation_id, r.finished_at_ms, r.admitted_at_ms
             FROM host_memory_reminders m
             JOIN host_automation_runs r ON r.automation_id = m.automation_id
             WHERE m.fired = 0 AND r.output IS NOT NULL AND r.status != ?1",
        )?
        .query_map([RunStatus::Skipped.as_str()], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, Option<i64>>(1)?,
                row.get::<_, i64>(2)?,
            ))
        })?
        .collect::<Result<Vec<_>, _>>()?;
    for (automation, finished_at_ms, admitted_at_ms) in finished {
        let automation = parse(&automation)?;
        let reminder = stored(db, automation)?.ok_or_else(|| {
            PluginError::Invalid(format!("reminder {automation} vanished while recorded"))
        })?;
        let at = local_time(zone, instant(finished_at_ms.unwrap_or(admitted_at_ms))?);
        let recorded = for_jobs(data_directory, reminder.principal)
            .map_err(unreadable)
            .and_then(|memory| match memory {
                Some(memory) => memory
                    .record_fired(&reminder.item, Some(reminder.due), at)
                    .map_err(unreadable),
                None => Ok(()),
            });
        if let Err(error) = recorded {
            unread(reminder.principal, &error);
            continue;
        }
        db.execute(
            "UPDATE host_memory_reminders SET fired = 1 WHERE automation_id = ?1",
            [automation.to_string()],
        )?;
    }
    Ok(())
}

/// Logs that `principal`'s notebook could not be read or written for their
/// reminders.
fn unread(principal: Uuid, error: &PluginError) {
    event(
        "warn",
        "memory_reminders_unread",
        &json!({"principal_id": principal, "error": error.to_string()}),
    );
}

/// The reminder `automation` is, if it is one.
fn stored(db: &Connection, automation: Uuid) -> Result<Option<Reminder>, PluginError> {
    let row = db
        .query_row(
            "SELECT principal_id, item_id, due FROM host_memory_reminders WHERE automation_id = ?1",
            [automation.to_string()],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                ))
            },
        )
        .optional()?;
    row.map(|(principal, item, due)| {
        Ok(Reminder {
            principal: parse(&principal)?,
            item,
            due: parse_due(&due)?,
        })
    })
    .transpose()
}

fn parse_due(value: &str) -> Result<DateTime, PluginError> {
    value
        .parse()
        .map_err(|error| PluginError::Invalid(format!("a reminder's due time: {error}")))
}

/// Whether the trigger `automation` has a run, finished or not, or was
/// deleted: it is not wanted again.
fn ran_or_deleted(db: &Connection, automation: Uuid) -> Result<bool, PluginError> {
    Ok(db.query_row(
        "SELECT EXISTS(SELECT 1 FROM host_automation_runs WHERE automation_id = ?1)
             OR EXISTS(SELECT 1 FROM host_automation_deletions WHERE automation_id = ?1)",
        [automation.to_string()],
        |row| row.get(0),
    )?)
}

fn parse(value: &str) -> Result<Uuid, PluginError> {
    Uuid::parse_str(value)
        .map_err(|error| PluginError::Invalid(format!("a stored reminder id: {error}")))
}

fn instant(at_ms: i64) -> Result<Timestamp, PluginError> {
    Timestamp::from_millisecond(at_ms)
        .map_err(|error| PluginError::Invalid(format!("a reminder time: {error}")))
}

fn unreadable(error: impl std::fmt::Display) -> PluginError {
    PluginError::Unavailable(format!(
        "the notebook could not be read for a reminder: {error}"
    ))
}
