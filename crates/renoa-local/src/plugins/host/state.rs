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

pub(crate) fn initialize(tx: &rusqlite::Transaction<'_>) -> rusqlite::Result<()> {
    tx.execute_batch("CREATE TABLE IF NOT EXISTS host_agent_builtin_plugins (
        agent_id TEXT NOT NULL REFERENCES host_agents(agent_id),
        plugin_id TEXT NOT NULL CHECK(plugin_id IN ('renoa.agents','renoa.routines','renoa.documents','renoa.skills','renoa.git')),
        enabled INTEGER NOT NULL CHECK(enabled IN (0,1)), PRIMARY KEY(agent_id,plugin_id)
    ) STRICT;
    CREATE TABLE IF NOT EXISTS host_builtin_plugin_operations (
        agent_id TEXT NOT NULL REFERENCES host_agents(agent_id), operation_id TEXT NOT NULL,
        request_json TEXT NOT NULL CHECK(json_valid(request_json)), result_json TEXT NOT NULL CHECK(json_valid(result_json)),
        PRIMARY KEY(agent_id,operation_id)
    ) STRICT;")
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
    tx.execute("INSERT INTO host_agent_builtin_plugins(agent_id,plugin_id,enabled) VALUES(?1,?2,?3) ON CONFLICT(agent_id,plugin_id) DO UPDATE SET enabled=excluded.enabled",params![agent.to_string(), plugin.id(), enabled])?;
    let result = HostPluginActivation {
        plugin_id: plugin.id().to_owned(),
        enabled,
    };
    tx.execute("INSERT INTO host_builtin_plugin_operations(agent_id,operation_id,request_json,result_json) VALUES(?1,?2,?3,?4)",params![agent.to_string(),operation,request,serde_json::to_string(&result)?])?;
    tx.commit()?;
    Ok(result)
}
