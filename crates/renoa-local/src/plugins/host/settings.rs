//! Per-agent settings of compiled Host plugins.
//!
//! The plugin validates its settings before anything is stored. A change
//! applies to messages admitted after it: each message freezes the context it
//! was admitted with, so settings stay outside the runtime digest.

use std::path::Path;

use renoa_kernel::AgentId;
use rusqlite::{OptionalExtension as _, params};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::{HostPluginId, time::TimeSettings};
use crate::{host::catalog, plugins::PluginError};

pub(crate) const SETTINGS_TABLE: &str = "CREATE TABLE IF NOT EXISTS host_agent_plugin_settings (
        agent_id TEXT NOT NULL REFERENCES host_agents(agent_id),
        plugin_id TEXT NOT NULL CHECK(plugin_id IN ('renoa.time')),
        settings_json TEXT NOT NULL
            CHECK(json_valid(settings_json) AND json_type(settings_json) = 'object'),
        PRIMARY KEY(agent_id, plugin_id)
    ) STRICT;";

/// One agent's stored settings for one compiled plugin; `{}` is the defaults.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct HostPluginSettings {
    pub plugin_id: String,
    pub settings: Value,
}

/// The settings a plugin accepts, in their stored form.
fn validate(plugin: HostPluginId, settings: Value) -> Result<Value, PluginError> {
    match plugin {
        HostPluginId::Time => Ok(serde_json::to_value(TimeSettings::validate(settings)?)?),
        HostPluginId::Agents
        | HostPluginId::Automations
        | HostPluginId::Documents
        | HostPluginId::Skills
        | HostPluginId::Git => Err(PluginError::Invalid(format!(
            "{} has no settings",
            plugin.id()
        ))),
    }
}

/// Whether a plugin takes settings. Derived from [`validate`], its one owner.
pub(crate) fn configurable(plugin: HostPluginId) -> bool {
    validate(plugin, Value::Object(serde_json::Map::new())).is_ok()
}

pub(crate) fn read_in(
    db: &rusqlite::Connection,
    agent: AgentId,
    plugin: HostPluginId,
) -> Result<Value, PluginError> {
    let stored: Option<String> = db
        .query_row(
            "SELECT settings_json FROM host_agent_plugin_settings WHERE agent_id=?1 AND plugin_id=?2",
            params![agent.to_string(), plugin.id()],
            |row| row.get(0),
        )
        .optional()?;
    stored.map_or_else(
        || Ok(Value::Object(serde_json::Map::new())),
        |json| Ok(serde_json::from_str(&json)?),
    )
}

pub(crate) fn read(
    path: &Path,
    agent: AgentId,
    plugin: HostPluginId,
) -> Result<Value, PluginError> {
    read_in(&catalog::open_verified(path)?, agent, plugin)
}

/// Replaces one agent's settings for `plugin`, once per operation identity.
pub(crate) fn configure(
    path: &Path,
    agent: AgentId,
    plugin: HostPluginId,
    settings: Value,
    operation: &str,
) -> Result<HostPluginSettings, PluginError> {
    let settings = validate(plugin, settings)?;
    let request = serde_json::to_string(&("configure", plugin.id(), &settings))?;
    let result = HostPluginSettings {
        plugin_id: plugin.id().to_owned(),
        settings,
    };
    let mut db = catalog::open_verified(path)?;
    let tx = db.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
    let receipt: Option<(String, String)> = tx
        .query_row(
            "SELECT request_json, result_json FROM host_builtin_plugin_operations
             WHERE agent_id=?1 AND operation_id=?2",
            params![agent.to_string(), operation],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()?;
    if let Some((previous, stored)) = receipt {
        if previous != request {
            return Err(PluginError::Conflict(
                "plugin operation identity was reused with different fields".to_owned(),
            ));
        }
        let stored: HostPluginSettings = serde_json::from_str(&stored)?;
        if stored != result {
            return Err(PluginError::Invalid("invalid plugin receipt".to_owned()));
        }
        return Ok(stored);
    }
    if result
        .settings
        .as_object()
        .is_some_and(serde_json::Map::is_empty)
    {
        tx.execute(
            "DELETE FROM host_agent_plugin_settings WHERE agent_id=?1 AND plugin_id=?2",
            params![agent.to_string(), plugin.id()],
        )?;
    } else {
        tx.execute(
            "INSERT INTO host_agent_plugin_settings(agent_id, plugin_id, settings_json)
             VALUES(?1, ?2, ?3)
             ON CONFLICT(agent_id, plugin_id) DO UPDATE SET settings_json=excluded.settings_json",
            params![
                agent.to_string(),
                plugin.id(),
                serde_json::to_string(&result.settings)?
            ],
        )?;
    }
    tx.execute(
        "INSERT INTO host_builtin_plugin_operations(agent_id, operation_id, request_json, result_json)
         VALUES(?1, ?2, ?3, ?4)",
        params![agent.to_string(), operation, request, serde_json::to_string(&result)?],
    )?;
    tx.commit()?;
    Ok(result)
}
