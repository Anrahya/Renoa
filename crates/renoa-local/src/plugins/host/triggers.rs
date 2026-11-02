//! The `triggers` contribution point: a schedule a compiled plugin owns.
//!
//! A plugin's trigger is a Host automation that no agent created. The
//! plugin says, from the catalog, which triggers it keeps, for which agent
//! and on what schedule ([`wanted`]); the Host's scheduler converges the
//! automations to that before it admits due runs, and hands each run of one
//! back to the plugin ([`run`]) instead of sending it to the agent as a
//! message. The exception is a `renoa.memory` reminder: its run is delivered
//! to its agent as a run of an automation the agent made would be, with the
//! submission the plugin decides when it is admitted ([`delivery`]). The
//! scheduler's coalescing and lateness rules hold for it as for any
//! automation, except that a trigger's one-time schedule may already be due
//! when it is converged: its plugin, not an agent, set the time.
//!
//! No agent lists, reads, changes, or runs a plugin's automation, and its
//! runs are not the agent's results: every agent-facing query leaves out
//! [`PLUGIN_OWNED`]. Host inspection shows it with its owner.
//!
//! Compiled contributors only: a trigger's run is first-party code driving
//! the agent's own journaled turns.

use std::path::PathBuf;

use renoa_kernel::AgentId;
use rusqlite::{Connection, OptionalExtension as _};
use uuid::Uuid;

use super::{
    HostPluginId,
    memory::{
        self,
        reminders::{Agendas, Reminder},
    },
};

use crate::{
    AutomationRun, AutomationSpec, LocalHost, LocalHostError, RunStatus, plugins::PluginError,
};
pub(crate) use memory::reminders::Delivery;

/// The automations plugins own, each for one agent. A row is kept, disabled,
/// when its plugin no longer wants it, so its runs stay with it.
pub(crate) const TRIGGERS_TABLE: &str = "CREATE TABLE IF NOT EXISTS host_plugin_triggers (
        automation_id TEXT PRIMARY KEY REFERENCES host_automations(id),
        plugin_id TEXT NOT NULL CHECK (plugin_id IN ('renoa.memory'))
    ) STRICT;";

/// The ids of the automations plugins own, for `NOT IN` in every query that
/// serves an agent.
pub(crate) const PLUGIN_OWNED: &str = "SELECT automation_id FROM host_plugin_triggers";

/// The ids of the automations plugins own whose runs the Host runs itself:
/// every one but a reminder, whose runs go to its agent.
pub(crate) const HOST_RUN: &str = "SELECT automation_id FROM host_plugin_triggers
    WHERE automation_id NOT IN (SELECT automation_id FROM host_memory_reminders)";

/// What plugins read for their triggers beyond the catalog: the data
/// directory the notebooks are in, and the agendas `renoa.memory` keeps
/// between convergences. The schedule's owner holds it.
pub(crate) struct TriggerContext {
    data_directory: PathBuf,
    agendas: Agendas,
}

impl TriggerContext {
    pub(crate) fn new(data_directory: PathBuf) -> Self {
        Self {
            data_directory,
            agendas: Agendas::default(),
        }
    }
}

/// One trigger a plugin keeps: the automation it is, what it should be, and,
/// for a reminder, whose agenda item it hands over.
pub(crate) struct Wanted {
    pub(crate) id: Uuid,
    pub(crate) spec: AutomationSpec,
    pub(crate) reminder: Option<Reminder>,
}

pub(crate) fn initialize(transaction: &rusqlite::Transaction<'_>) -> rusqlite::Result<()> {
    transaction.execute_batch(TRIGGERS_TABLE)
}

/// The triggers `plugin` keeps as of `now_ms`; every other one it owns is
/// disabled.
pub(crate) fn wanted(
    plugin: HostPluginId,
    db: &Connection,
    context: &TriggerContext,
    now_ms: i64,
) -> Result<Vec<Wanted>, PluginError> {
    match plugin {
        HostPluginId::Memory => {
            let timers = [
                memory::nightly::wanted(db, now_ms)?
                    .map(|spec| (automation_id(plugin, spec.agent_id), spec)),
                memory::note_pass::wanted(db)?,
            ];
            let reminders =
                memory::reminders::wanted(db, &context.data_directory, &context.agendas, now_ms)?;
            Ok(timers
                .into_iter()
                .flatten()
                .map(|(id, spec)| Wanted {
                    id,
                    spec,
                    reminder: None,
                })
                .chain(reminders.into_iter().map(|(id, spec, reminder)| Wanted {
                    id,
                    spec,
                    reminder: Some(reminder),
                }))
                .collect())
        }
        HostPluginId::Agents
        | HostPluginId::Automations
        | HostPluginId::Documents
        | HostPluginId::Skills
        | HostPluginId::Git
        | HostPluginId::Time => Ok(Vec::new()),
    }
}

/// The automation `plugin`'s first trigger is for `agent`.
pub(crate) fn automation_id(plugin: HostPluginId, agent: AgentId) -> Uuid {
    crate::stable_id::stable_id(&format!("renoa.plugin.trigger.v1:{}:{agent}", plugin.id()))
}

/// The automation `plugin`'s trigger `name` is for `agent`.
pub(crate) fn automation_id_named(plugin: HostPluginId, agent: AgentId, name: &str) -> Uuid {
    crate::stable_id::stable_id(&format!(
        "renoa.plugin.trigger.v1:{}:{agent}:{name}",
        plugin.id()
    ))
}

/// Records that `plugin` owns `automation`, and, for a reminder, whose item
/// it hands over, inside the transaction that creates it.
pub(crate) fn own(
    transaction: &rusqlite::Transaction<'_>,
    automation: Uuid,
    plugin: HostPluginId,
    reminder: Option<&Reminder>,
) -> rusqlite::Result<()> {
    transaction.execute(
        "INSERT INTO host_plugin_triggers(automation_id, plugin_id) VALUES (?1, ?2)",
        [automation.to_string(), plugin.id().to_owned()],
    )?;
    reminder.map_or(Ok(()), |reminder| {
        memory::reminders::own(transaction, automation, reminder)
    })
}

/// Whether the runs of `automation`, a plugin's trigger, are delivered to its
/// agent rather than run by its plugin.
pub(crate) fn delivers(db: &Connection, automation: Uuid) -> rusqlite::Result<bool> {
    db.query_row(
        "SELECT EXISTS(SELECT 1 FROM host_memory_reminders WHERE automation_id = ?1)",
        [automation.to_string()],
        |row| row.get(0),
    )
}

/// How the run of `automation` for `agent` admitted at `now_ms` is
/// delivered to the agent; `None` for an automation whose submission is its
/// standing task.
///
/// # Errors
///
/// Returns the plugin's failure to decide; the run is not admitted.
pub(crate) fn delivery(
    db: &Connection,
    context: &TriggerContext,
    automation: Uuid,
    agent: AgentId,
    now_ms: i64,
) -> Result<Option<Delivery>, PluginError> {
    memory::reminders::delivery(db, &context.data_directory, automation, agent, now_ms)
}

/// Every automation a plugin owns, with its owner.
pub(crate) fn owned(db: &Connection) -> Result<Vec<(Uuid, HostPluginId)>, PluginError> {
    db.prepare("SELECT automation_id, plugin_id FROM host_plugin_triggers ORDER BY automation_id")?
        .query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })?
        .map(|row| {
            let (automation, plugin) = row?;
            Ok((parse_automation(&automation)?, parse_plugin(&plugin)?))
        })
        .collect()
}

/// The plugin that runs `automation`'s runs itself, if one does: its owner,
/// unless they are delivered to its agent.
pub(crate) fn runner(
    db: &Connection,
    automation: Uuid,
) -> Result<Option<HostPluginId>, PluginError> {
    db.query_row(
        &format!("SELECT plugin_id FROM host_plugin_triggers WHERE automation_id = ?1 AND automation_id IN ({HOST_RUN})"),
        [automation.to_string()],
        |row| row.get::<_, String>(0),
    )
    .optional()?
    .map(|plugin| parse_plugin(&plugin))
    .transpose()
}

/// How a plugin's run ended. `failed_tool_calls` is `None` for a run that
/// never executed.
#[derive(Debug)]
pub(crate) struct Ended {
    pub(crate) status: RunStatus,
    pub(crate) output: String,
    pub(crate) failed_tool_calls: Option<u32>,
}

/// Runs one admitted run of `plugin`'s trigger, as of `now_ms`.
///
/// # Errors
///
/// Returns an error only when the run must be resumed rather than ended;
/// every outcome the plugin decides is an [`Ended`].
pub(crate) async fn run(
    host: &LocalHost,
    plugin: HostPluginId,
    run: &AutomationRun,
    now_ms: i64,
) -> Result<Ended, LocalHostError> {
    match plugin {
        HostPluginId::Memory if run.automation_id == memory::note_pass::timer_id(run.agent_id) => {
            memory::note_pass::run(host, run, now_ms).await
        }
        HostPluginId::Memory => memory::nightly::run(host, run, now_ms).await,
        HostPluginId::Agents
        | HostPluginId::Automations
        | HostPluginId::Documents
        | HostPluginId::Skills
        | HostPluginId::Git
        | HostPluginId::Time => Ok(Ended {
            status: RunStatus::Failed,
            output: format!("{} owns no trigger", plugin.id()),
            failed_tool_calls: None,
        }),
    }
}

fn parse_automation(value: &str) -> Result<Uuid, PluginError> {
    Uuid::parse_str(value)
        .map_err(|error| PluginError::Invalid(format!("stored trigger automation id: {error}")))
}

fn parse_plugin(value: &str) -> Result<HostPluginId, PluginError> {
    HostPluginId::parse(value)
        .ok_or_else(|| PluginError::Invalid(format!("stored trigger owner `{value}`")))
}
