//! Schema 41 moves turn timing from agent behavior to the `renoa.time` plugin.
//!
//! Behavior loses `turn_timing` in every agent row and in every receipt that
//! stores a whole definition. An agent that had timing off gets `renoa.time`
//! turned off; every other agent keeps the plugin's default, on, in the Host's
//! zone as before. The compiled-plugin table is rebuilt to accept
//! `renoa.time`, and the plugin settings table is created.

use rusqlite::Transaction;

use super::HostCatalogError;
use crate::plugins::host::{settings::SETTINGS_TABLE, state::BUILTIN_PLUGINS_TABLE};

pub(super) fn move_turn_timing_to_plugin(
    transaction: &Transaction<'_>,
) -> Result<(), HostCatalogError> {
    transaction.execute_batch(
        "ALTER TABLE host_agent_builtin_plugins RENAME TO host_agent_builtin_plugins_v40;",
    )?;
    transaction.execute_batch(BUILTIN_PLUGINS_TABLE)?;
    transaction.execute_batch(
        "INSERT INTO host_agent_builtin_plugins(agent_id, plugin_id, enabled)
            SELECT agent_id, plugin_id, enabled FROM host_agent_builtin_plugins_v40;
         DROP TABLE host_agent_builtin_plugins_v40;
         INSERT INTO host_agent_builtin_plugins(agent_id, plugin_id, enabled)
            SELECT agent_id, 'renoa.time', 0 FROM host_agents
            WHERE json_extract(operational_json, '$.behavior.turn_timing') = 'off';
         UPDATE host_agents
            SET operational_json = json_remove(operational_json, '$.behavior.turn_timing');
         UPDATE host_agent_creations
            SET result_json = json_remove(result_json, '$.operational.behavior.turn_timing');
         UPDATE host_agent_renames
            SET result_json = json_remove(result_json, '$.operational.behavior.turn_timing');",
    )?;
    transaction.execute_batch(SETTINGS_TABLE)?;
    Ok(())
}

/// Turns a current catalog back into the schema 40 shape, so upgrade tests
/// start from the tables an earlier runtime wrote.
#[cfg(test)]
pub(crate) fn restore_schema_40(connection: &rusqlite::Connection) {
    connection
        .execute_batch(
            "DROP TABLE host_agent_plugin_settings;
             DELETE FROM host_agent_builtin_plugins WHERE plugin_id = 'renoa.time';
             ALTER TABLE host_agent_builtin_plugins RENAME TO host_agent_builtin_plugins_v41;
             CREATE TABLE host_agent_builtin_plugins (
                agent_id TEXT NOT NULL REFERENCES host_agents(agent_id),
                plugin_id TEXT NOT NULL CHECK(plugin_id IN ('renoa.agents','renoa.automations','renoa.documents','renoa.skills','renoa.git')),
                enabled INTEGER NOT NULL CHECK(enabled IN (0,1)), PRIMARY KEY(agent_id,plugin_id)
             ) STRICT;
             INSERT INTO host_agent_builtin_plugins SELECT * FROM host_agent_builtin_plugins_v41;
             DROP TABLE host_agent_builtin_plugins_v41;",
        )
        .expect("restore the schema 40 plugin tables");
}
