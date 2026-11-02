//! The one message a pass sends: Mnem's, rendered by `Memory::note_pass` from
//! the conversation's range, the notes written since it began, and the
//! agent's automations as data. Renoa adds nothing to it.

use std::path::Path;

use jiff::{
    Timestamp,
    civil::{Date, DateTime},
    tz::TimeZone,
};
use mnem::{
    memory::Memory,
    pass::{self, PassInput, PassTools, Scheduled},
};
use renoa_kernel::AgentId;
use rusqlite::Connection;

use super::{
    super::{NOTE, RECALL, local_time, read_only},
    store::Conversation,
};
use crate::plugins::{PluginError, host::triggers::PLUGIN_OWNED};

/// The pass message for `conversation` of `agent` at `now` in `zone`, in
/// the pass day `day`: the one ending at the nightly sweep, which runs in
/// the next.
pub(super) fn message(
    db: &Connection,
    data_directory: &Path,
    agent: AgentId,
    conversation: &Conversation,
    zone: &TimeZone,
    (day, now): (Date, DateTime),
) -> Result<String, PluginError> {
    let armed = armed(db, agent, zone)?;
    let ended = ended(db, agent, zone, day)?;
    let since = conversation
        .range_since_ms
        .map(|at| Ok::<_, PluginError>(local_time(zone, instant(at)?)))
        .transpose()?;
    let render = |memory: &Memory| {
        let notes = memory.len().map_err(unreadable)?;
        memory
            .note_pass(&PassInput {
                since,
                // A notebook reset since the range began starts it again.
                since_note: conversation.range_since_note.min(notes),
                armed: &armed,
                ended: &ended,
                day,
                now,
                tools: PassTools {
                    note: NOTE,
                    recall: RECALL,
                },
            })
            .map_err(unreadable)
    };
    if let Some(memory) = read_only(data_directory, conversation.principal).map_err(unreadable)? {
        return render(&memory);
    }
    // A person with no notebook yet has nothing noted; the pass's first note
    // creates it, so nothing is created to render the message.
    let scratch = tempfile::tempdir().map_err(|source| PluginError::Io {
        action: "create",
        path: std::env::temp_dir(),
        source,
    })?;
    render(&Memory::create(scratch.path()).map_err(unreadable)?)
}

/// The notebook's length for `principal`; 0 without one.
pub(super) fn notes(data_directory: &Path, principal: uuid::Uuid) -> Result<u64, PluginError> {
    match read_only(data_directory, principal).map_err(unreadable)? {
        Some(memory) => memory.len().map_err(unreadable),
        None => Ok(0),
    }
}

/// The agent's own automations still to run, with when they are next due.
fn armed(db: &Connection, agent: AgentId, zone: &TimeZone) -> Result<Vec<Scheduled>, PluginError> {
    let rows = db
        .prepare(&format!(
            "SELECT id, name, next_due_ms, json_extract(schedule_json, '$.kind')
             FROM host_automations a
             WHERE agent_id = ?1 AND enabled = 1 AND id NOT IN ({PLUGIN_OWNED})
               AND NOT EXISTS (SELECT 1 FROM host_automation_deletions d WHERE d.automation_id = a.id)
             ORDER BY next_due_ms"
        ))?
        .query_map([agent.to_string()], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, i64>(2)?,
                row.get::<_, String>(3)?,
            ))
        })?
        .collect::<Result<Vec<_>, _>>()?;
    rows.into_iter()
        .map(|(id, name, due, kind)| {
            Ok(Scheduled {
                id,
                name,
                due: Some(local_time(zone, instant(due)?)),
                status: if kind == "once" { "armed" } else { "repeating" }.to_owned(),
            })
        })
        .collect()
}

/// The agent's own automations whose latest run finished since the pass day
/// `day` began, with how it ended. A deleted automation's runs go with it
/// once none is in flight (see `retention::purge_deleted`), so a deletion is
/// not listed.
fn ended(
    db: &Connection,
    agent: AgentId,
    zone: &TimeZone,
    day: Date,
) -> Result<Vec<Scheduled>, PluginError> {
    let start = day
        .at(pass::DAY_START_HOUR, 0, 0, 0)
        .to_zoned(zone.clone())
        .map_err(|error| PluginError::Invalid(format!("the pass day's start: {error}")))?
        .timestamp()
        .as_millisecond();
    let rows = db
        .prepare(&format!(
            "SELECT a.id, a.name, r.due_ms, r.status
             FROM host_automation_runs r JOIN host_automations a ON a.id = r.automation_id
             WHERE a.agent_id = ?1 AND a.id NOT IN ({PLUGIN_OWNED}) AND r.finished_at_ms >= ?2
               AND r.sequence = (SELECT max(x.sequence) FROM host_automation_runs x
                                 WHERE x.automation_id = a.id AND x.output IS NOT NULL)
             ORDER BY r.due_ms"
        ))?
        .query_map(rusqlite::params![agent.to_string(), start], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, i64>(2)?,
                row.get::<_, String>(3)?,
            ))
        })?
        .collect::<Result<Vec<_>, _>>()?;
    rows.into_iter()
        .map(|(id, name, due, status)| {
            Ok(Scheduled {
                id,
                name,
                due: Some(local_time(zone, instant(due)?)),
                status,
            })
        })
        .collect()
}

fn instant(at_ms: i64) -> Result<Timestamp, PluginError> {
    Timestamp::from_millisecond(at_ms)
        .map_err(|error| PluginError::Invalid(format!("an automation time: {error}")))
}

fn unreadable(error: impl std::fmt::Display) -> PluginError {
    PluginError::Unavailable(format!(
        "the notebook could not be read for a pass: {error}"
    ))
}
