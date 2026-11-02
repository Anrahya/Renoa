//! The `triggers` contribution point: a schedule a compiled plugin owns.
//!
//! A plugin's trigger is a Host automation that no agent created. The
//! plugin says, from the catalog, which agent it runs for and on what
//! schedule ([`wanted`]); the Host's scheduler converges the automation to
//! that before it admits due runs, and hands each run of it back to the
//! plugin ([`run`]) instead of sending it to the agent as a message. The
//! scheduler's coalescing and lateness rules hold for it as for any cron
//! automation.
//!
//! No agent lists, reads, changes, or runs a plugin's automation, and its
//! runs are not the agent's results: every agent-facing query leaves out
//! [`PLUGIN_OWNED`]. Host inspection shows it with its owner.
//!
//! Compiled contributors only: a trigger's run is first-party code driving
//! the agent's own journaled turns.

use renoa_kernel::AgentId;
use rusqlite::{Connection, OptionalExtension as _};
use uuid::Uuid;

use super::{HostPluginId, memory};
use crate::{
    AutomationRun, AutomationSpec, LocalHost, LocalHostError, RunStatus, plugins::PluginError,
};

/// The automations plugins own, one per plugin and agent it ever ran for. A
/// row is kept, disabled, when its plugin stops running for that agent, so
/// its runs stay with it.
pub(crate) const TRIGGERS_TABLE: &str = "CREATE TABLE IF NOT EXISTS host_plugin_triggers (
        automation_id TEXT PRIMARY KEY REFERENCES host_automations(id),
        plugin_id TEXT NOT NULL CHECK (plugin_id IN ('renoa.memory'))
    ) STRICT;";

/// The ids of the automations plugins own, for `NOT IN` in every query that
/// serves an agent.
pub(crate) const PLUGIN_OWNED: &str = "SELECT automation_id FROM host_plugin_triggers";

pub(crate) fn initialize(transaction: &rusqlite::Transaction<'_>) -> rusqlite::Result<()> {
    transaction.execute_batch(TRIGGERS_TABLE)
}

/// What `plugin`'s trigger should be as of `now_ms`: the automation it runs,
/// enabled, for one agent; `None` while it runs for no agent.
pub(crate) fn wanted(
    plugin: HostPluginId,
    db: &Connection,
    now_ms: i64,
) -> Result<Option<AutomationSpec>, PluginError> {
    match plugin {
        HostPluginId::Memory => memory::nightly::wanted(db, now_ms),
        HostPluginId::Agents
        | HostPluginId::Automations
        | HostPluginId::Documents
        | HostPluginId::Skills
        | HostPluginId::Git
        | HostPluginId::Time => Ok(None),
    }
}

/// The automation `plugin`'s trigger is for `agent`.
pub(crate) fn automation_id(plugin: HostPluginId, agent: AgentId) -> Uuid {
    crate::stable_id::stable_id(&format!("renoa.plugin.trigger.v1:{}:{agent}", plugin.id()))
}

/// Records that `plugin` owns `automation`, inside the transaction that
/// creates it.
pub(crate) fn own(
    transaction: &rusqlite::Transaction<'_>,
    automation: Uuid,
    plugin: HostPluginId,
) -> rusqlite::Result<()> {
    transaction
        .execute(
            "INSERT INTO host_plugin_triggers(automation_id, plugin_id) VALUES (?1, ?2)",
            [automation.to_string(), plugin.id().to_owned()],
        )
        .map(drop)
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

/// The plugin that owns `automation`, if one does.
pub(crate) fn owner(
    db: &Connection,
    automation: Uuid,
) -> Result<Option<HostPluginId>, PluginError> {
    db.query_row(
        "SELECT plugin_id FROM host_plugin_triggers WHERE automation_id = ?1",
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
