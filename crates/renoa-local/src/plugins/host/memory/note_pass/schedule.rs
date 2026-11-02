//! When a conversation's pass is due, from Mnem's `pass::due`, and the timer
//! that asks it again: the trigger this plugin keeps at the earliest time any
//! main-agent conversation is to be asked, which the scheduler converges.

use jiff::{
    Timestamp,
    civil::{Date, DateTime},
    tz::TimeZone,
};
use mnem::pass::{self, PassDecision, PassKind, PassStats};
use rusqlite::Connection;
use uuid::Uuid;

use super::{
    super::{local_time, nightly::driver, notebook_zone},
    Kind,
    store::{self, Conversation},
};
use crate::{
    AutomationSchedule, AutomationSpec,
    plugins::{PluginError, host::HostPluginId, host::triggers},
};

const TIMER_NAME: &str = "renoa.memory note pass";
const TIMER_TASK: &str = "Ask each quiet conversation of the main agent whether its memory pass is due, and run the passes that are.";

/// The automation the note pass timer is for `agent`.
pub(crate) fn timer_id(agent: renoa_kernel::AgentId) -> Uuid {
    triggers::automation_id_named(HostPluginId::Memory, agent, "note-pass")
}

/// The timer, for the nightly run's driver, at the earliest time one of its
/// conversations is to be asked again; `None` while none is.
pub(crate) fn wanted(db: &Connection) -> Result<Option<(Uuid, AutomationSpec)>, PluginError> {
    let Some(agent) = driver(db)? else {
        return Ok(None);
    };
    let Some(at) = store::next_check(db, agent)? else {
        return Ok(None);
    };
    Ok(Some((
        timer_id(agent),
        AutomationSpec {
            agent_id: agent,
            name: TIMER_NAME.to_owned(),
            prompt: TIMER_TASK.to_owned(),
            schedule: AutomationSchedule::Once {
                at: instant(at)?.to_string(),
            },
            enabled: true,
        },
    )))
}

/// The notebook zone, and the wall time there at `at_ms`.
pub(super) fn zone_and_time(
    db: &Connection,
    at_ms: i64,
) -> Result<(TimeZone, DateTime), PluginError> {
    let zone = notebook_zone(db)?;
    let now = local_time(&zone, instant(at_ms)?);
    Ok((zone, now))
}

/// Mnem's decision on a pass of `kind` in `conversation` as of `now_ms`,
/// counting the person's passes in `day`.
pub(super) fn decide(
    db: &Connection,
    conversation: &Conversation,
    kind: Kind,
    day: Date,
    now_ms: i64,
) -> Result<PassDecision, PluginError> {
    let (zone, now) = zone_and_time(db, now_ms)?;
    let (last_pass, passes_today) = store::person(db, conversation.principal, &day.to_string())?;
    let stats = PassStats {
        last_activity: local_time(&zone, instant(conversation.last_activity_ms)?),
        last_pass: last_pass
            .map(|at| Ok::<_, PluginError>(local_time(&zone, instant(at)?)))
            .transpose()?,
        passes_today,
        user_chars: conversation.user_chars,
    };
    Ok(pass::due(&stats, kind.mnem(), now))
}

/// Asks again whether `session`'s daytime pass is due as of `now_ms`, and
/// [`follow`]s the decision. Nothing for a conversation the pass does not
/// follow.
pub(super) fn reschedule(db: &Connection, session: Uuid, now_ms: i64) -> Result<(), PluginError> {
    let Some(conversation) = store::conversation(db, session)? else {
        return Ok(());
    };
    let (_, now) = zone_and_time(db, now_ms)?;
    let decision = decide(db, &conversation, Kind::Daytime, pass::day(now), now_ms)?;
    follow(db, session, decision, now_ms)
}

/// Sets when `session` is asked again after `decision` as of `now_ms`: now
/// for a pass to run, the time Mnem names for one to wait, and never, until
/// new activity, for one Mnem skips.
pub(super) fn follow(
    db: &Connection,
    session: Uuid,
    decision: PassDecision,
    now_ms: i64,
) -> Result<(), PluginError> {
    let at = match decision {
        PassDecision::Run => Some(now_ms),
        PassDecision::Wait { until } => Some(at_ms(db, until)?),
        PassDecision::Skip { .. } => None,
    };
    store::check_at(db, session, at)?;
    Ok(())
}

/// The instant of `wall`, a wall time in the notebook zone.
fn at_ms(db: &Connection, wall: DateTime) -> Result<i64, PluginError> {
    Ok(wall
        .to_zoned(notebook_zone(db)?)
        .map_err(|error| PluginError::Invalid(format!("a pass time: {error}")))?
        .timestamp()
        .as_millisecond())
}

/// The pass day the nightly run due at `due_ms` sweeps: the one that ends at
/// that night's [`pass::DAY_START_HOUR`], not the one it starts, whose first
/// slot the sweep must not take. A run due before that hour, as a fallback
/// zone's after a change of offset can be, sweeps the day still running.
pub(super) fn ending_day(db: &Connection, due_ms: i64) -> Result<Date, PluginError> {
    let (_, due) = zone_and_time(db, due_ms)?;
    let day = pass::day(due);
    if due.hour() < pass::DAY_START_HOUR {
        return Ok(day);
    }
    day.yesterday()
        .map_err(|error| PluginError::Invalid(format!("the day before {day}: {error}")))
}

impl Kind {
    const fn mnem(self) -> PassKind {
        match self {
            Self::Daytime => PassKind::Daytime,
            Self::Sweep => PassKind::Sweep,
        }
    }
}

fn instant(at_ms: i64) -> Result<Timestamp, PluginError> {
    Timestamp::from_millisecond(at_ms)
        .map_err(|error| PluginError::Invalid(format!("a pass time: {error}")))
}
