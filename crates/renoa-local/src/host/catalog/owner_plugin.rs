//! Schema 48 admits the `renoa.owner` plugin and gives the feed what its
//! `ask_owner` and `notify_owner` items carry.
//!
//! The compiled-plugin table is rebuilt so its `CHECK` accepts `renoa.owner`,
//! and every stored activation is copied unchanged. No row is added: an agent
//! without one has the plugin's default, on. Every feed item gains an
//! urgency, `whenever` for those already stored, and an ask its choices.

use rusqlite::Transaction;

use super::HostCatalogError;
use crate::plugins::host::state::admit_every_plugin;

pub(super) fn admit_owner_plugin(transaction: &Transaction<'_>) -> Result<(), HostCatalogError> {
    admit_every_plugin(transaction)?;
    crate::host::feed::add_urgency(transaction)
}

/// Turns a current catalog back into the schema 48 shape: its items keep no
/// record of a wake.
#[cfg(test)]
pub(crate) fn restore_schema_48(connection: &rusqlite::Connection) {
    connection
        .execute_batch("ALTER TABLE host_feed_items DROP COLUMN wake_sent_at_ms;")
        .expect("restore the schema 48 feed");
}

/// Turns a current catalog back into the schema 47 shape, so upgrade tests
/// start from the tables an earlier runtime wrote.
#[cfg(test)]
pub(crate) fn restore_schema_47(connection: &rusqlite::Connection) {
    restore_schema_48(connection);
    connection
        .execute_batch(
            "DELETE FROM host_agent_builtin_plugins WHERE plugin_id = 'renoa.owner';
             ALTER TABLE host_agent_builtin_plugins RENAME TO host_agent_builtin_plugins_v48;
             CREATE TABLE host_agent_builtin_plugins (
                agent_id TEXT NOT NULL REFERENCES host_agents(agent_id),
                plugin_id TEXT NOT NULL CHECK(plugin_id IN ('renoa.agents','renoa.automations','renoa.documents','renoa.skills','renoa.git','renoa.time','renoa.memory')),
                enabled INTEGER NOT NULL CHECK(enabled IN (0,1)), PRIMARY KEY(agent_id,plugin_id)
             ) STRICT;
             INSERT INTO host_agent_builtin_plugins SELECT * FROM host_agent_builtin_plugins_v48;
             DROP TABLE host_agent_builtin_plugins_v48;
             DELETE FROM host_feed_items WHERE kind IN ('ask', 'update');
             ALTER TABLE host_feed_items DROP COLUMN choices;
             ALTER TABLE host_feed_items DROP COLUMN urgency;",
        )
        .expect("restore the schema 47 tables");
}
