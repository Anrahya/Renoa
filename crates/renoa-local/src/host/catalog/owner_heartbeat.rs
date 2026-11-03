//! Schema 50 lets every compiled plugin keep settings and triggers, for
//! `renoa.owner`'s heartbeat: its `heartbeat_hours` setting and the trigger
//! it keeps for each agent.
//!
//! Both tables are rebuilt so their `CHECK` accepts every compiled plugin,
//! and every stored row is copied unchanged, the reminders that refer to the
//! triggers included. No row is added: no agent has a heartbeat until it is
//! set.
//! The reset's cutover runs it on any catalog.

use rusqlite::Transaction;

use super::HostCatalogError;
use crate::plugins::host::{
    memory::reminders, settings::settings_table, state::rebuild, triggers::triggers_table,
};

pub(super) fn admit_every_plugin_trigger(
    transaction: &Transaction<'_>,
) -> Result<(), HostCatalogError> {
    rebuild(transaction, "host_agent_plugin_settings", &settings_table())?;
    // The reminders refer to the triggers, so they wait outside while the
    // triggers are rebuilt and come back as they were.
    transaction.execute_batch(
        "CREATE TEMP TABLE reminders_v49 AS SELECT * FROM host_memory_reminders;
         DROP TABLE host_memory_reminders;",
    )?;
    rebuild(transaction, "host_plugin_triggers", &triggers_table())?;
    reminders::initialize(transaction)?;
    transaction.execute_batch(
        "INSERT INTO host_memory_reminders SELECT * FROM temp.reminders_v49;
         DROP TABLE temp.reminders_v49;",
    )?;
    Ok(())
}

/// Turns a current catalog back into the schema 49 shape: only `renoa.time`
/// keeps settings and only `renoa.memory` triggers.
#[cfg(test)]
pub(crate) fn restore_schema_49(connection: &rusqlite::Connection) {
    crate::host::automations::follow_up::restore_schema_50(connection);
    connection
        .execute_batch(
            "PRAGMA foreign_keys = OFF;
             PRAGMA legacy_alter_table = ON;
             DELETE FROM host_agent_plugin_settings WHERE plugin_id != 'renoa.time';
             ALTER TABLE host_agent_plugin_settings RENAME TO host_agent_plugin_settings_v50;
             CREATE TABLE host_agent_plugin_settings (
                agent_id TEXT NOT NULL REFERENCES host_agents(agent_id),
                plugin_id TEXT NOT NULL CHECK(plugin_id IN ('renoa.time')),
                settings_json TEXT NOT NULL
                    CHECK(json_valid(settings_json) AND json_type(settings_json) = 'object'),
                PRIMARY KEY(agent_id, plugin_id)
             ) STRICT;
             INSERT INTO host_agent_plugin_settings SELECT * FROM host_agent_plugin_settings_v50;
             DROP TABLE host_agent_plugin_settings_v50;
             CREATE TEMP TABLE heartbeats AS
                SELECT automation_id FROM host_plugin_triggers WHERE plugin_id != 'renoa.memory';
             DELETE FROM host_automation_runs WHERE automation_id IN (SELECT automation_id FROM heartbeats);
             DELETE FROM host_plugin_triggers WHERE automation_id IN (SELECT automation_id FROM heartbeats);
             DELETE FROM host_automations WHERE id IN (SELECT automation_id FROM heartbeats);
             DROP TABLE heartbeats;
             ALTER TABLE host_plugin_triggers RENAME TO host_plugin_triggers_v50;
             CREATE TABLE host_plugin_triggers (
                automation_id TEXT PRIMARY KEY REFERENCES host_automations(id),
                plugin_id TEXT NOT NULL CHECK (plugin_id IN ('renoa.memory'))
             ) STRICT;
             INSERT INTO host_plugin_triggers SELECT * FROM host_plugin_triggers_v50;
             DROP TABLE host_plugin_triggers_v50;
             PRAGMA legacy_alter_table = OFF;",
        )
        .expect("restore the schema 49 tables");
}
