//! `renoa.owner`'s heartbeat: a check-in of the agent's own every few hours
//! of the waking day, in which it reviews what happened and decides whether
//! the owner should hear of it.
//!
//! The heartbeat is this plugin's trigger (see `triggers`), one per agent,
//! while `renoa.owner` is on for it and its `heartbeat_hours` setting is set
//! and not 0: the owner sets it, or asks the agent to. Unlike the memory plugin's runs, a heartbeat's run is sent to its agent
//! as a message, on a task of the trigger's own, and quietly: what it writes
//! there wakes no one, so only its `notify_owner` and `ask_owner` calls
//! reach the owner. `set_today` works only in one of its runs ([`is_run`]).
//!
//! This is not the scheduler's liveness heartbeat (`host_automation_scheduler`).

use renoa_kernel::{AgentId, CommandId};
use rusqlite::{Connection, OptionalExtension as _};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use uuid::Uuid;

use super::{ASK, NOTIFY, TODAY};
use crate::{
    AutomationSchedule, AutomationSpec,
    plugins::{
        PluginError,
        host::{HostPluginId, settings, state, time::TimeSettings, triggers},
    },
};

const NAME: &str = "Heartbeat";

/// The first and last hour of the waking day a heartbeat runs in, in the
/// agent's zone.
const FIRST_HOUR: u8 = 8;
const LAST_HOUR: u8 = 22;

/// The most hours between two heartbeats: one a day.
const MOST_HOURS: u8 = 24;

/// The settings `renoa.owner` accepts. An absent `heartbeat_hours` is the
/// default: no heartbeat.
#[derive(Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(crate) struct OwnerSettings {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    heartbeat_hours: Option<u8>,
}

impl OwnerSettings {
    /// Validates settings from a caller, before anything is stored.
    pub(crate) fn validate(value: Value) -> Result<Self, PluginError> {
        let invalid = || {
            PluginError::Invalid(format!(
                "renoa.owner takes {{\"heartbeat_hours\": n}}: how many hours apart the heartbeat runs between {FIRST_HOUR}:00 and {LAST_HOUR}:00, a whole number from 1 to {MOST_HOURS}, or 0 to turn it off."
            ))
        };
        let settings: Self = serde_json::from_value(value).map_err(|_| invalid())?;
        if settings
            .heartbeat_hours
            .is_some_and(|hours| hours > MOST_HOURS)
        {
            return Err(invalid());
        }
        Ok(settings)
    }

    /// These settings with the heartbeat `hours` apart; 0 is off.
    pub(crate) fn with_heartbeat(hours: u8) -> Result<Value, PluginError> {
        Ok(serde_json::to_value(Self::validate(serde_json::json!({
            "heartbeat_hours": hours
        }))?)?)
    }
}

/// How many hours apart `agent`'s heartbeat runs, from its stored settings;
/// 0 is off. It holds whether or not `renoa.owner` is on for the agent.
pub(crate) fn hours(db: &Connection, agent: AgentId) -> Result<u8, PluginError> {
    let stored = OwnerSettings::validate(settings::read_in(db, agent, HostPluginId::Owner)?)?;
    Ok(stored.heartbeat_hours.unwrap_or(0))
}

/// The heartbeats wanted: one for each agent with `renoa.owner` on and a
/// heartbeat set, at minute 0 every [`hours`] from [`FIRST_HOUR`] to
/// [`LAST_HOUR`] in the agent's zone. A zone without an IANA name keeps the
/// hours in UTC.
pub(crate) fn wanted(db: &Connection) -> Result<Vec<(Uuid, AutomationSpec)>, PluginError> {
    let agents = db
        .prepare("SELECT agent_id FROM host_agents ORDER BY agent_id")?
        .query_map([], |row| row.get::<_, String>(0))?
        .collect::<Result<Vec<_>, _>>()?;
    let mut wanted = Vec::new();
    for agent in agents {
        let agent = AgentId::from_uuid(Uuid::parse_str(&agent).map_err(|error| {
            PluginError::Invalid(format!("stored agent id `{agent}`: {error}"))
        })?);
        let hours = hours(db, agent)?;
        if hours == 0 || !state::enabled_in(db, agent, HostPluginId::Owner)? {
            continue;
        }
        let zone = TimeSettings::zone_of(db, Some(agent))?;
        wanted.push((
            triggers::automation_id(HostPluginId::Owner, agent),
            AutomationSpec {
                agent_id: agent,
                name: NAME.to_owned(),
                prompt: task(),
                schedule: AutomationSchedule::Cron {
                    expression: format!("0 {FIRST_HOUR}-{LAST_HOUR}/{hours} * * *"),
                    timezone: zone.iana_name().unwrap_or("UTC").to_owned(),
                },
                enabled: true,
            },
        ));
    }
    Ok(wanted)
}

/// The heartbeat's standing task.
fn task() -> String {
    format!(
        "This is your heartbeat: a check-in nobody asked for, in a conversation nobody reads. Look at what happened since your last heartbeat: your automations' results, failed and skipped ones first, and the follow-ups you set. Then call {TODAY} with one sentence about the owner's day for their home screen. If something deserves the owner's attention, make one {NOTIFY} call, or {ASK} when you need their decision. Then end your turn without writing anything."
    )
}

/// Whether `command` is a run of `agent`'s heartbeat: a run's command has
/// the run's id.
pub(crate) fn is_run(
    db: &Connection,
    agent: AgentId,
    command: CommandId,
) -> rusqlite::Result<bool> {
    Ok(db
        .query_row(
            "SELECT 1 FROM host_automation_runs WHERE id = ?1 AND agent_id = ?2 AND automation_id = ?3",
            [
                command.to_string(),
                agent.to_string(),
                triggers::automation_id(HostPluginId::Owner, agent).to_string(),
            ],
            |_| Ok(()),
        )
        .optional()?
        .is_some())
}
