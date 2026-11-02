//! Schema 47 gives `renoa.memory` its reminders: which of its triggers hands
//! over which item of a person's agenda.
//!
//! It starts empty: the scheduler creates the reminders the first time it
//! converges the triggers.

use rusqlite::Transaction;

use super::HostCatalogError;
use crate::plugins::host::memory::reminders;

pub(super) fn add_reminders(transaction: &Transaction<'_>) -> Result<(), HostCatalogError> {
    reminders::initialize(transaction)?;
    Ok(())
}

/// Turns a current catalog back into the schema 46 shape, so upgrade tests
/// start from the tables an earlier runtime wrote. A schema 46 runtime kept
/// no reminders, so their triggers go too.
#[cfg(test)]
pub(crate) fn restore_schema_46(connection: &rusqlite::Connection) {
    connection
        .execute_batch(
            "CREATE TEMP TABLE reminder_triggers AS
                SELECT automation_id FROM host_memory_reminders;
             DROP TABLE host_memory_reminders;
             DELETE FROM host_automation_runs
                WHERE automation_id IN (SELECT automation_id FROM reminder_triggers);
             DELETE FROM host_automation_conversation_deletions
                WHERE automation_id IN (SELECT automation_id FROM reminder_triggers);
             DELETE FROM host_automation_deletions
                WHERE automation_id IN (SELECT automation_id FROM reminder_triggers);
             DELETE FROM host_plugin_triggers
                WHERE automation_id IN (SELECT automation_id FROM reminder_triggers);
             DELETE FROM host_automations
                WHERE id IN (SELECT automation_id FROM reminder_triggers);
             DROP TABLE reminder_triggers;",
        )
        .expect("restore the schema 46 tables");
}
