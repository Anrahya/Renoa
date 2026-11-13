use std::path::Path;

use renoa_kernel::AgentId;
use rusqlite::{OptionalExtension as _, params};
use serde::{Deserialize, Serialize};

use super::HostPluginId;
use crate::{host::catalog, plugins::PluginError};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct HostPluginActivation {
    pub plugin_id: String,
    pub enabled: bool,
}

/// The `CHECK` list naming every [`HostPluginId`], for the tables keyed by a
/// compiled plugin.
pub(crate) fn every_plugin() -> String {
    HostPluginId::ALL
        .map(|plugin| format!("'{}'", plugin.id()))
        .join(",")
}

/// Which compiled plugins each agent turned on or off; no row is the default, on.
/// The table, whose `CHECK` accepts every [`HostPluginId`].
pub(crate) fn builtin_plugins_table() -> String {
    format!(
        "CREATE TABLE IF NOT EXISTS host_agent_builtin_plugins (
        agent_id TEXT NOT NULL REFERENCES host_agents(agent_id),
        plugin_id TEXT NOT NULL CHECK(plugin_id IN ({})),
        enabled INTEGER NOT NULL CHECK(enabled IN (0,1)), PRIMARY KEY(agent_id,plugin_id)
    ) STRICT;",
        every_plugin()
    )
}

/// Rebuilds the table so its `CHECK` accepts every [`HostPluginId`], copying
/// every stored activation unchanged. No row is added: an agent without one
/// has a plugin's default, on.
pub(crate) fn admit_every_plugin(tx: &rusqlite::Transaction<'_>) -> rusqlite::Result<()> {
    rebuild(tx, "host_agent_builtin_plugins", &builtin_plugins_table())
}

/// Rebuilds `table` as `create` defines it now, copying every row unchanged;
/// the columns stay as they were. A table that refers to `table` must be set
/// aside first: the rename makes it refer to the old copy, and dropping that
/// copy deletes its rows.
pub(crate) fn rebuild(
    tx: &rusqlite::Transaction<'_>,
    table: &str,
    create: &str,
) -> rusqlite::Result<()> {
    tx.execute_batch(&format!("ALTER TABLE {table} RENAME TO {table}_previous;"))?;
    tx.execute_batch(create)?;
    tx.execute_batch(&format!(
        "INSERT INTO {table} SELECT * FROM {table}_previous;
         DROP TABLE {table}_previous;"
    ))
}

pub(crate) fn initialize(tx: &rusqlite::Transaction<'_>) -> rusqlite::Result<()> {
    tx.execute_batch(&builtin_plugins_table())?;
    tx.execute_batch("CREATE TABLE IF NOT EXISTS host_builtin_plugin_operations (
        agent_id TEXT NOT NULL REFERENCES host_agents(agent_id), operation_id TEXT NOT NULL,
        request_json TEXT NOT NULL CHECK(json_valid(request_json)), result_json TEXT NOT NULL CHECK(json_valid(result_json)),
        PRIMARY KEY(agent_id,operation_id)
    ) STRICT;")?;
    tx.execute_batch(&super::settings::settings_table())
}

pub(crate) fn enabled(
    path: &Path,
    agent: AgentId,
    plugin: HostPluginId,
) -> Result<bool, PluginError> {
    let db = catalog::open_verified(path)?;
    Ok(enabled_in(&db, agent, plugin)?)
}

pub(crate) fn enabled_in(
    db: &rusqlite::Connection,
    agent: AgentId,
    plugin: HostPluginId,
) -> rusqlite::Result<bool> {
    let value = db
        .query_row(
            "SELECT enabled FROM host_agent_builtin_plugins WHERE agent_id=?1 AND plugin_id=?2",
            params![agent.to_string(), plugin.id()],
            |row| row.get::<_, bool>(0),
        )
        .optional()?;
    Ok(value.unwrap_or(true))
}

/// Starts a new agent with `plugin` turned off, inside its creation.
pub(crate) fn turn_off(
    tx: &rusqlite::Transaction<'_>,
    agent: AgentId,
    plugin: HostPluginId,
) -> rusqlite::Result<()> {
    tx.execute(
        "INSERT INTO host_agent_builtin_plugins(agent_id,plugin_id,enabled) VALUES(?1,?2,0)",
        params![agent.to_string(), plugin.id()],
    )
    .map(drop)
}

/// Applies a preset's built-in plugin defaults: any plugin not listed in
/// `host_plugins_off` reverts to default enabled, and listed ones turn off.
pub(crate) fn apply_preset_defaults(
    tx: &rusqlite::Transaction<'_>,
    agent: AgentId,
    host_plugins_off: &[HostPluginId],
) -> rusqlite::Result<()> {
    tx.execute(
        "DELETE FROM host_agent_builtin_plugins WHERE agent_id = ?1",
        params![agent.to_string()],
    )?;
    for plugin in host_plugins_off {
        turn_off(tx, agent, *plugin)?;
    }
    Ok(())
}

pub(crate) fn change(
    path: &Path,
    agent: AgentId,
    plugin: HostPluginId,
    enabled: bool,
    operation: &str,
) -> Result<HostPluginActivation, PluginError> {
    let mut db = catalog::open_verified(path)?;
    let tx = db.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
    let request = serde_json::to_string(&(plugin.id(), enabled))?;
    let receipt: Option<(String, String)> = tx.query_row("SELECT request_json,result_json FROM host_builtin_plugin_operations WHERE agent_id=?1 AND operation_id=?2",
        params![agent.to_string(), operation], |row| Ok((row.get(0)?,row.get(1)?))).optional()?;
    if let Some((previous, result)) = receipt {
        if previous != request {
            return Err(PluginError::Conflict(
                "plugin operation identity was reused with different fields".to_owned(),
            ));
        }
        let result: HostPluginActivation = serde_json::from_str(&result)?;
        if result.plugin_id != plugin.id() || result.enabled != enabled {
            return Err(PluginError::Invalid("invalid plugin receipt".to_owned()));
        }
        return Ok(result);
    }
    // Only `plugin_manage` reaches this, for the calling agent itself, so
    // turning a plugin back on is the agent's own act.
    if enabled && !enabled_in(&tx, agent, plugin)? {
        crate::host::feed::ability_turned_on(
            &tx,
            &agent.to_string(),
            crate::host::feed::Ability::Compiled(plugin),
            &format!("compiled:{operation}"),
        )?;
    }
    tx.execute("INSERT INTO host_agent_builtin_plugins(agent_id,plugin_id,enabled) VALUES(?1,?2,?3) ON CONFLICT(agent_id,plugin_id) DO UPDATE SET enabled=excluded.enabled",params![agent.to_string(), plugin.id(), enabled])?;
    let result = HostPluginActivation {
        plugin_id: plugin.id().to_owned(),
        enabled,
    };
    tx.execute("INSERT INTO host_builtin_plugin_operations(agent_id,operation_id,request_json,result_json) VALUES(?1,?2,?3,?4)",params![agent.to_string(),operation,request,serde_json::to_string(&result)?])?;
    tx.commit()?;
    Ok(result)
}
