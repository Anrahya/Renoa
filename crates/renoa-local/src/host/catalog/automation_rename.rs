//! Schema 34 renamed routines to automations.
//!
//! This is migration code, so it names the routine tables and plugin id that
//! schemas 16 through 33 used. It runs inside the upgrade transaction: a
//! catalog without the expected tables fails the upgrade and keeps its schema.

use rusqlite::Transaction;

use super::HostCatalogError;

/// Renames the routine tables, columns, and index in place, and moves stored
/// Host plugin activations from `renoa.routines` to `renoa.automations`.
pub(super) fn rename_routines(transaction: &Transaction<'_>) -> Result<(), HostCatalogError> {
    transaction.execute_batch(
        "DROP INDEX host_routine_pending;
         ALTER TABLE host_routines RENAME TO host_automations;
         ALTER TABLE host_routine_deletions RENAME TO host_automation_deletions;
         ALTER TABLE host_automation_deletions RENAME COLUMN routine_id TO automation_id;
         ALTER TABLE host_routine_mutations RENAME TO host_automation_mutations;
         ALTER TABLE host_routine_owner_mutations RENAME TO host_automation_owner_mutations;
         ALTER TABLE host_routine_runs RENAME TO host_automation_runs;
         ALTER TABLE host_automation_runs RENAME COLUMN routine_id TO automation_id;
         ALTER TABLE host_agent_builtin_plugins RENAME TO host_agent_builtin_plugins_v33;",
    )?;
    // The current definitions own the new index and the plugin id CHECK.
    crate::host::automations::initialize(transaction)?;
    crate::plugins::host::state::initialize(transaction)?;
    transaction.execute_batch(
        "INSERT INTO host_agent_builtin_plugins(agent_id, plugin_id, enabled)
         SELECT agent_id,
                CASE plugin_id WHEN 'renoa.routines' THEN 'renoa.automations' ELSE plugin_id END,
                enabled
         FROM host_agent_builtin_plugins_v33;
         DROP TABLE host_agent_builtin_plugins_v33;
         UPDATE host_builtin_plugin_operations
         SET request_json = replace(request_json, '\"renoa.routines\"', '\"renoa.automations\"'),
             result_json = replace(result_json, '\"renoa.routines\"', '\"renoa.automations\"');",
    )?;
    Ok(())
}

/// Turns a current catalog's automation tables back into the schema 33 routine
/// tables, so upgrade tests start from the shape an earlier runtime wrote.
#[cfg(test)]
pub(crate) fn restore_routine_tables(connection: &rusqlite::Connection) {
    connection
        .execute_batch(
            "DROP INDEX host_automation_pending;
             ALTER TABLE host_automations RENAME TO host_routines;
             ALTER TABLE host_automation_deletions RENAME COLUMN automation_id TO routine_id;
             ALTER TABLE host_automation_deletions RENAME TO host_routine_deletions;
             ALTER TABLE host_automation_mutations RENAME TO host_routine_mutations;
             ALTER TABLE host_automation_owner_mutations RENAME TO host_routine_owner_mutations;
             ALTER TABLE host_automation_runs RENAME COLUMN automation_id TO routine_id;
             ALTER TABLE host_automation_runs RENAME TO host_routine_runs;
             CREATE INDEX host_routine_pending ON host_routine_runs(sequence) WHERE output IS NULL;
             ALTER TABLE host_agent_builtin_plugins RENAME TO host_agent_builtin_plugins_v34;
             CREATE TABLE host_agent_builtin_plugins (
                agent_id TEXT NOT NULL REFERENCES host_agents(agent_id),
                plugin_id TEXT NOT NULL CHECK(plugin_id IN ('renoa.agents','renoa.routines','renoa.documents','renoa.skills','renoa.git')),
                enabled INTEGER NOT NULL CHECK(enabled IN (0,1)), PRIMARY KEY(agent_id,plugin_id)
             ) STRICT;
             INSERT INTO host_agent_builtin_plugins
             SELECT agent_id,
                    CASE plugin_id WHEN 'renoa.automations' THEN 'renoa.routines' ELSE plugin_id END,
                    enabled
             FROM host_agent_builtin_plugins_v34;
             DROP TABLE host_agent_builtin_plugins_v34;
             UPDATE host_builtin_plugin_operations
             SET request_json = replace(request_json, '\"renoa.automations\"', '\"renoa.routines\"'),
                 result_json = replace(result_json, '\"renoa.automations\"', '\"renoa.routines\"');",
        )
        .expect("restore the schema 33 routine tables");
}
