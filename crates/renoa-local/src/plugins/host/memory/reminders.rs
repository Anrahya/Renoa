//! `renoa.memory`'s reminders: each timed item on a person's agenda that is
//! to be told to them, a nudge in `MIND.md` or an `[at …]` in a note,
//! becomes a one-time automation of the main agent at its due time.
//!
//! Mnem decides what is on the agenda (`Memory::agenda`), whether an item is
//! still to fire when its time comes and the one message its run sends
//! (`Memory::nudge_run`), and what fired (`Memory::record_fired`). The Host
//! decides when and where each runs:
//!
//! 1. Before the scheduler opens its write transaction, it reads and writes
//!    the notebooks (see `notebook`): each person's timed items, read again
//!    only when the notebook's `AgendaStamp` changes; the fired record of
//!    each reminder whose run succeeded; and Mnem's message for each one due.
//! 2. It then converges the reminders (see `triggers`): one trigger per
//!    person, item, due time and attempt, with the id derived from them, for
//!    the people the main agent has conversations with, for the items due
//!    within [`HORIZON_MS`]. A replay converges, and an item Mnem no longer
//!    lists is no longer wanted. A quiet item is never a trigger: its run
//!    would have no tools and no conversation the person does not see, so
//!    it stays unfired on the agenda for the nightly Dream.
//! 3. At most [`OVERDUE_MAX`] overdue reminders of one person run in
//!    [`CATCH_UP_MS`], the latest first; the rest wait for a later window or
//!    for a Dream to drop them.
//! 4. Its run is delivered to the main agent in its latest conversation with
//!    the person, with Mnem's message as the whole submission. One Mnem says
//!    is no longer due is skipped; one whose message was not read yet waits
//!    for the next tick.
//! 5. A run that succeeded is recorded fired, and only then marked so; Mnem
//!    records an item and due time once, so a crash between the two repeats
//!    nothing. A run that failed is tried once more [`RETRY_AFTER_MS`] after
//!    it ended; a retry that fails too leaves the item unfired, and is
//!    logged. A trigger that ran is not wanted again.
//! 6. A trigger no longer wanted is deleted once no run of it is left, and
//!    removed once its conversation is gone (see `retention`).

use std::{
    cmp::Reverse,
    collections::HashMap,
    path::Path,
    sync::{LazyLock, Mutex, PoisonError},
    time::Instant,
};

use jiff::{Timestamp, civil::DateTime, tz::TimeZone};
use mnem::memory::NudgeRun;
use renoa_kernel::AgentId;
use rusqlite::Connection;
use serde_json::json;
use uuid::Uuid;

use super::{nightly::driver, note_pass, notebook_zone, pass::event};
use crate::{
    AutomationSchedule, AutomationSpec, RunStatus,
    host::automations::Throttle,
    plugins::{
        PluginError,
        host::{HostPluginId, triggers},
    },
};

mod notebook;
mod store;

pub(crate) use notebook::Agendas;
use store::{Outcome, State};
pub(crate) use store::{initialize, is_run};

const NAME: &str = "renoa.memory reminder";
const TASK: &str =
    "When an item on the person's agenda is due, hand it to the main agent in Mnem's words.";

/// Why a reminder's run is skipped.
const OFF: &str = "renoa.memory is off for this agent, or it is no longer the Host's main agent.";
const GONE: &str =
    "No longer due: it fired, went stale, or a later dream or note dropped or moved it.";
const QUIET: &str = "It is no longer to be told to the person; it waits for the nightly Dream.";
const NO_CONVERSATION: &str =
    "The main agent no longer has a conversation with this person to tell them in.";

const MINUTE_MS: i64 = 60_000;
/// How long after a failed run ended its one retry is due.
const RETRY_AFTER_MS: i64 = 15 * MINUTE_MS;
/// How far ahead an item becomes a trigger.
const HORIZON_MS: i64 = 48 * 60 * MINUTE_MS;
/// A reminder due longer ago than this is overdue, and its run is late.
const LATE_MS: i64 = 15 * MINUTE_MS;
/// The window the overdue reminders of one person are counted in.
const CATCH_UP_MS: i64 = 12 * 60 * MINUTE_MS;
/// The most overdue reminders of one person run in [`CATCH_UP_MS`].
const OVERDUE_MAX: usize = 3;

/// One timed item of a person's agenda, due at a wall time in the notebook
/// zone; `notify` is false for one not to be told to them.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Item {
    id: String,
    due: DateTime,
    notify: bool,
}

/// One item of one person's agenda, handed over on attempt 0, or 1 for the
/// retry after a failed run.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Reminder {
    principal: Uuid,
    item: String,
    due: DateTime,
    attempt: u8,
}

/// What [`prepare`] read and recorded in the notebooks, for the convergence
/// and admission that follow it.
#[derive(Default)]
pub(crate) struct Prepared {
    /// Reminders whose runs settled, to mark so.
    settled: Vec<(Uuid, Outcome)>,
    /// Each person's timed items; `None` when their notebook cannot be read.
    people: HashMap<Uuid, Option<Vec<Item>>>,
    /// Mnem's run for each person's item and due time due now; `None` once
    /// it is no longer due.
    runs: HashMap<(Uuid, String, DateTime), Option<NudgeRun>>,
}

/// How a reminder's run is delivered, decided when it is admitted.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Delivery {
    /// Mnem's `message`, in the person's conversation `origin`.
    Send { message: String, origin: Uuid },
    /// Not run, for this reason.
    Skip(&'static str),
    /// Not admitted yet: its message is read before the next admission.
    Later,
}

/// Reads and records in the notebooks what the next convergence and
/// admission need, outside the catalog's write transaction.
///
/// # Errors
///
/// Returns catalog and time failures.
pub(crate) fn prepare(
    db: &Connection,
    data_directory: &Path,
    agendas: &Agendas,
    now_ms: i64,
) -> Result<Prepared, PluginError> {
    notebook::prepare(db, data_directory, agendas, now_ms)
}

/// The reminders the main agent keeps as of `now_ms`, after marking what
/// `prepared` settled, as the module describes. A person whose notebook
/// cannot be read keeps the reminders they have; an item whose due time
/// cannot be scheduled is logged and left out, so it holds up no other.
///
/// # Errors
///
/// Returns catalog and time failures; the scheduler keeps every trigger as
/// it was and asks again.
pub(crate) fn wanted(
    db: &Connection,
    prepared: &Prepared,
    now_ms: i64,
) -> Result<Vec<(Uuid, AutomationSpec, Reminder)>, PluginError> {
    for (automation, outcome) in &prepared.settled {
        store::settle(db, *automation, *outcome)?;
    }
    let Some(agent) = driver(db)? else {
        return Ok(Vec::new());
    };
    let zone = notebook_zone(db)?;
    let states = store::states(db, agent)?;
    let late = store::late_runs(db, now_ms - CATCH_UP_MS, LATE_MS)?;
    let mut wanted = Vec::new();
    for (principal, _) in note_pass::latest_conversations(db, agent)? {
        let items = match prepared.people.get(&principal) {
            Some(Some(items)) => items.clone(),
            Some(None) | None => store::kept(db, agent, principal)?,
        };
        let mut overdue = Vec::new();
        for item in items.iter().filter(|item| item.notify) {
            let (reminder, at_ms) = match candidate(agent, principal, item, &zone, &states) {
                Ok(Some(candidate)) => candidate,
                Ok(None) => continue,
                Err(error) => {
                    report(
                        "memory_reminder_unschedulable",
                        principal,
                        &error.to_string(),
                    );
                    continue;
                }
            };
            if at_ms > now_ms + HORIZON_MS {
                continue;
            }
            if at_ms < now_ms - LATE_MS {
                overdue.push((reminder, at_ms));
            } else {
                wanted.push(trigger(agent, reminder, at_ms)?);
            }
        }
        overdue.sort_by_key(|(_, at_ms)| Reverse(*at_ms));
        overdue.truncate(OVERDUE_MAX.saturating_sub(late.get(&principal).copied().unwrap_or(0)));
        for (reminder, at_ms) in overdue {
            wanted.push(trigger(agent, reminder, at_ms)?);
        }
    }
    Ok(wanted)
}

/// The reminder `item` of `principal` is wanted as, with when it is due:
/// its first attempt at its due time until that ran, then, after a failed
/// run, its retry; nothing once a run of it succeeded, was skipped or is in
/// flight, or once the retry ran.
///
/// # Errors
///
/// Returns a due time with no instant in the notebook zone.
fn candidate(
    agent: AgentId,
    principal: Uuid,
    item: &Item,
    zone: &TimeZone,
    states: &HashMap<Uuid, State>,
) -> Result<Option<(Reminder, i64)>, PluginError> {
    let reminder = |attempt| Reminder {
        principal,
        item: item.id.clone(),
        due: item.due,
        attempt,
    };
    let first = reminder(0);
    Ok(match states.get(&id(agent, &first)) {
        Some(state) if state.deleted => None,
        None => Some((due_ms(&first, zone)?, first)),
        Some(state) if !state.ran => Some((due_ms(&first, zone)?, first)),
        Some(State {
            status: Some(RunStatus::Failed),
            ended_at_ms: Some(ended_at_ms),
            ..
        }) => {
            let retry = reminder(1);
            match states.get(&id(agent, &retry)) {
                Some(state) if state.ran || state.deleted => None,
                Some(_) | None => Some((ended_at_ms + RETRY_AFTER_MS, retry)),
            }
        }
        Some(_) => None,
    }
    .map(|(at_ms, reminder)| (reminder, at_ms)))
}

/// When `reminder`'s item is due, as an instant.
fn due_ms(reminder: &Reminder, zone: &TimeZone) -> Result<i64, PluginError> {
    let due = reminder.due.to_zoned(zone.clone()).map_err(|error| {
        PluginError::Invalid(format!(
            "item {} is due at {}, which has no time in the notebook zone: {error}",
            reminder.item, reminder.due
        ))
    })?;
    Ok(due.timestamp().as_millisecond())
}

/// The trigger `agent` keeps for `reminder`, due at `at_ms`.
fn trigger(
    agent: AgentId,
    reminder: Reminder,
    at_ms: i64,
) -> Result<(Uuid, AutomationSpec, Reminder), PluginError> {
    let spec = AutomationSpec {
        agent_id: agent,
        name: NAME.to_owned(),
        prompt: TASK.to_owned(),
        schedule: AutomationSchedule::Once {
            at: instant(at_ms)?.to_string(),
        },
        enabled: true,
    };
    Ok((id(agent, &reminder), spec, reminder))
}

/// The automation `agent` keeps for `reminder`.
fn id(agent: AgentId, reminder: &Reminder) -> Uuid {
    let Reminder {
        principal,
        item,
        due,
        attempt,
    } = reminder;
    let retry = if *attempt == 0 { "" } else { "#retry" };
    triggers::automation_id_named(
        HostPluginId::Memory,
        agent,
        &format!(
            "reminder:{principal}:{item}@{}{retry}",
            mnem::fired::minutes(*due)
        ),
    )
}

/// Records that the trigger `automation`, being created, is `reminder`.
pub(crate) fn own(
    transaction: &rusqlite::Transaction<'_>,
    automation: Uuid,
    reminder: &Reminder,
) -> rusqlite::Result<()> {
    store::own(transaction, automation, reminder)
}

/// How the run of `automation` for `agent` is delivered, from what
/// `prepared` read; `None` when it is no reminder.
///
/// # Errors
///
/// Returns catalog failures; the run is not admitted.
pub(crate) fn delivery(
    db: &Connection,
    prepared: &Prepared,
    automation: Uuid,
    agent: AgentId,
) -> Result<Option<Delivery>, PluginError> {
    let Some(reminder) = store::stored(db, automation)? else {
        return Ok(None);
    };
    if driver(db)? != Some(agent) {
        return Ok(Some(Delivery::Skip(OFF)));
    }
    let key = (reminder.principal, reminder.item.clone(), reminder.due);
    let Some(run) = prepared.runs.get(&key) else {
        return Ok(Some(Delivery::Later));
    };
    let Some(run) = run else {
        return Ok(Some(Delivery::Skip(GONE)));
    };
    if !run.notify {
        return Ok(Some(Delivery::Skip(QUIET)));
    }
    let conversation = note_pass::latest_conversations(db, agent)?
        .into_iter()
        .find_map(|(principal, session)| (principal == reminder.principal).then_some(session));
    Ok(Some(conversation.map_or(
        Delivery::Skip(NO_CONVERSATION),
        |origin| Delivery::Send {
            message: run.message.clone(),
            origin,
        },
    )))
}

/// Logs that `principal`'s notebook could not be read or written for their
/// reminders, as [`report`] does.
fn unread(principal: Uuid, error: &PluginError) {
    report("memory_reminders_unread", principal, &error.to_string());
}

/// Logs `name` for `principal` with `error`, at most as often as the
/// scheduler logs a row it passes over, since it repeats each tick.
fn report(name: &'static str, principal: Uuid, error: &str) {
    static LOGGED: LazyLock<Mutex<Throttle>> = LazyLock::new(Mutex::default);
    let met = LOGGED.lock().unwrap_or_else(PoisonError::into_inner).met(
        &format!("{name}:{principal}"),
        error,
        Instant::now(),
    );
    if let Some(met) = met {
        event(
            "warn",
            name,
            &json!({"principal_id": principal, "error": error, "met": met}),
        );
    }
}

fn instant(at_ms: i64) -> Result<Timestamp, PluginError> {
    Timestamp::from_millisecond(at_ms)
        .map_err(|error| PluginError::Invalid(format!("a reminder time: {error}")))
}
