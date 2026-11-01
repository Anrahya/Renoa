//! Schema 43 admits the `renoa.memory` plugin.
//!
//! The compiled-plugin table is rebuilt so its `CHECK` accepts `renoa.memory`,
//! and every stored activation is copied unchanged. No row is added: an agent
//! without one has the plugin's default, on, so every existing agent gains
//! memory, as a new one does.

use rusqlite::Transaction;

use super::HostCatalogError;
use crate::plugins::host::state::BUILTIN_PLUGINS_TABLE;

pub(super) fn admit_memory_plugin(transaction: &Transaction<'_>) -> Result<(), HostCatalogError> {
    transaction.execute_batch(
        "ALTER TABLE host_agent_builtin_plugins RENAME TO host_agent_builtin_plugins_v42;",
    )?;
    transaction.execute_batch(BUILTIN_PLUGINS_TABLE)?;
    transaction.execute_batch(
        "INSERT INTO host_agent_builtin_plugins(agent_id, plugin_id, enabled)
            SELECT agent_id, plugin_id, enabled FROM host_agent_builtin_plugins_v42;
         DROP TABLE host_agent_builtin_plugins_v42;",
    )?;
    Ok(())
}

/// Turns a current catalog back into the schema 42 shape, so upgrade tests
/// start from the tables an earlier runtime wrote.
#[cfg(test)]
pub(crate) fn restore_schema_42(connection: &rusqlite::Connection) {
    connection
        .execute_batch(
            "DELETE FROM host_agent_builtin_plugins WHERE plugin_id = 'renoa.memory';
             ALTER TABLE host_agent_builtin_plugins RENAME TO host_agent_builtin_plugins_v43;
             CREATE TABLE host_agent_builtin_plugins (
                agent_id TEXT NOT NULL REFERENCES host_agents(agent_id),
                plugin_id TEXT NOT NULL CHECK(plugin_id IN ('renoa.agents','renoa.automations','renoa.documents','renoa.skills','renoa.git','renoa.time')),
                enabled INTEGER NOT NULL CHECK(enabled IN (0,1)), PRIMARY KEY(agent_id,plugin_id)
             ) STRICT;
             INSERT INTO host_agent_builtin_plugins SELECT * FROM host_agent_builtin_plugins_v43;
             DROP TABLE host_agent_builtin_plugins_v43;",
        )
        .expect("restore the schema 42 plugin table");
}
