//! Schema 45 gives `renoa.memory`'s note pass the conversations it follows
//! and the passes it ran.
//!
//! Both start empty: a conversation is followed from the person's next
//! message in it, and its range then begins at the notebook's length.

use rusqlite::Transaction;

use super::HostCatalogError;
use crate::plugins::host::memory::note_pass::store;

pub(super) fn add_note_passes(transaction: &Transaction<'_>) -> Result<(), HostCatalogError> {
    store::initialize(transaction)?;
    Ok(())
}

/// Turns a current catalog back into the schema 44 shape, so upgrade tests
/// start from the tables an earlier runtime wrote. A schema 44 runtime kept
/// no note pass timer, so its trigger goes too.
#[cfg(test)]
pub(crate) fn restore_schema_44(connection: &rusqlite::Connection) {
    super::restore_schema_45(connection);
    connection
        .execute_batch(
            "DROP TABLE host_memory_passes;
             DROP TABLE host_memory_conversations;
             CREATE TEMP TABLE note_pass_timers AS
                SELECT t.automation_id FROM host_plugin_triggers t
                JOIN host_automations a ON a.id = t.automation_id
                WHERE a.name = 'renoa.memory note pass';
             DELETE FROM host_automation_runs
                WHERE automation_id IN (SELECT automation_id FROM note_pass_timers);
             DELETE FROM host_plugin_triggers
                WHERE automation_id IN (SELECT automation_id FROM note_pass_timers);
             DELETE FROM host_automations
                WHERE id IN (SELECT automation_id FROM note_pass_timers);
             DROP TABLE note_pass_timers;",
        )
        .expect("restore the schema 44 tables");
}
