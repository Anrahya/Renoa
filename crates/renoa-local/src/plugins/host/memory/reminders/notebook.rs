//! The reminders' reads and writes of people's notebooks, made before the
//! scheduler opens its catalog write transaction, so Mnem's file locks and
//! reads never hold the catalog: each person's timed items, cached by their
//! notebook's `AgendaStamp`; the fired record of each run that succeeded;
//! and Mnem's message for each item due now.

use std::{collections::HashMap, path::Path, sync::Mutex};

use jiff::civil::DateTime;
use mnem::{
    heartbeat::NudgeInput,
    memory::{AgendaStamp, NudgeRun},
    mind::When,
};
use serde_json::json;
use uuid::Uuid;

use super::{
    Item, LATE_MS, Prepared, Reminder,
    store::{self, Outcome},
    unread,
};
use crate::{
    RunStatus,
    plugins::{
        PluginError,
        host::memory::{
            NOTE, for_jobs, local_time, nightly::driver, note_pass, notebook_zone, pass::event,
            read_only,
        },
    },
};

/// Each person's timed items as of their notebook's [`AgendaStamp`], read
/// again only when it changes. It lives as long as the schedule's owner, so
/// a restart reads each agenda once more.
#[derive(Default)]
pub(crate) struct Agendas(Mutex<HashMap<Uuid, (AgendaStamp, Vec<Item>)>>);

impl Agendas {
    /// The timed items of `principal`'s agenda, overdue ones included, as
    /// Mnem lists them at `now`; none without a notebook.
    fn timed(
        &self,
        data_directory: &Path,
        principal: Uuid,
        now: DateTime,
    ) -> Result<Vec<Item>, PluginError> {
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
                When::At(due) => Some(Item {
                    id: item.id,
                    due,
                    notify: item.notify,
                }),
                When::NextSession | When::Event(_) => None,
            })
            .collect();
        people.insert(principal, (stamp, items.clone()));
        Ok(items)
    }
}

/// Reads and records, from the catalog `db` as it stands at `now_ms`, what
/// the next convergence and admission need from notebooks. A notebook that
/// cannot be read or written is logged and left for the next tick: its
/// person's items are `None`, a fired record waits, and a due reminder is
/// not admitted yet.
///
/// # Errors
///
/// Returns catalog and time failures.
pub(super) fn prepare(
    db: &rusqlite::Connection,
    data_directory: &Path,
    agendas: &Agendas,
    now_ms: i64,
) -> Result<Prepared, PluginError> {
    let zone = notebook_zone(db)?;
    let now = local_time(&zone, super::instant(now_ms)?);
    let mut prepared = Prepared::default();
    for finished in store::unsettled(db)? {
        let reminder = &finished.reminder;
        match finished.status {
            RunStatus::Succeeded => {
                let at = local_time(&zone, super::instant(finished.at_ms)?);
                match record(data_directory, reminder, at) {
                    Ok(()) => prepared.settled.push((finished.automation, Outcome::Fired)),
                    Err(error) => unread(reminder.principal, &error),
                }
            }
            RunStatus::Failed => {
                if reminder.attempt > 0 {
                    event(
                        "warn",
                        "memory_reminder_given_up",
                        &json!({"principal_id": reminder.principal, "automation_id": finished.automation}),
                    );
                }
                prepared
                    .settled
                    .push((finished.automation, Outcome::Failed));
            }
            RunStatus::Skipped => {}
        }
    }
    let Some(agent) = driver(db)? else {
        return Ok(prepared);
    };
    // Due now: each armed reminder whose time came, and each item that
    // came due in the last [`LATE_MS`] and is no reminder yet, which this
    // convergence creates and may admit at once.
    let mut due: Vec<_> = store::due(db, now_ms)?
        .into_iter()
        .map(|(_, reminder)| (reminder.principal, reminder.item, reminder.due))
        .collect();
    let fresh_since = local_time(&zone, super::instant(now_ms - LATE_MS)?);
    for (principal, _) in note_pass::latest_conversations(db, agent)? {
        let items = agendas
            .timed(data_directory, principal, now)
            .map_err(|error| unread(principal, &error))
            .ok();
        for item in items.iter().flatten() {
            if item.notify
                && (fresh_since..=now).contains(&item.due)
                && !store::known(db, principal, &item.id, item.due)?
            {
                due.push((principal, item.id.clone(), item.due));
            }
        }
        prepared.people.insert(principal, items);
    }
    for key in due {
        if prepared.runs.contains_key(&key) {
            continue;
        }
        match message(data_directory, &key, now) {
            Ok(run) => {
                prepared.runs.insert(key, run);
            }
            Err(error) => unread(key.0, &error),
        }
    }
    Ok(prepared)
}

/// Records `reminder`'s item fired at `at`; nothing without a notebook.
fn record(data_directory: &Path, reminder: &Reminder, at: DateTime) -> Result<(), PluginError> {
    let Some(memory) = for_jobs(data_directory, reminder.principal).map_err(unreadable)? else {
        return Ok(());
    };
    memory
        .record_fired(&reminder.item, Some(reminder.due), at)
        .map_err(unreadable)
}

/// Mnem's message for the run of `principal`'s `item` due at `due`, at
/// `now`, or `None` once it is no longer due.
fn message(
    data_directory: &Path,
    (principal, item, due): &(Uuid, String, DateTime),
    now: DateTime,
) -> Result<Option<NudgeRun>, PluginError> {
    let Some(memory) = read_only(data_directory, *principal).map_err(unreadable)? else {
        return Ok(None);
    };
    memory
        .nudge_run(&NudgeInput {
            id: item,
            due: *due,
            now,
            note_tool: NOTE,
        })
        .map_err(unreadable)
}

fn unreadable(error: impl std::fmt::Display) -> PluginError {
    PluginError::Unavailable(format!(
        "the notebook could not be read or written for a reminder: {error}"
    ))
}
