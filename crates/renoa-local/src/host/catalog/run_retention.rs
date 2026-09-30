//! Schemas 39 and 40 bound automation data. Run history gains the indexes its
//! retention and reads use (39), deleted automations whose own conversation
//! may remain are recorded for the schedule's owner to delete (40), and
//! automations deleted before deletion removed their data are purged now,
//! except any with a run still in flight, which its finish purges. A schema 39
//! catalog already purged them, so this only records their conversations.

use rusqlite::Transaction;

use super::HostCatalogError;
use crate::host::automations;

pub(super) fn bound_run_history(
    transaction: &Transaction<'_>,
) -> Result<Vec<automations::Removed>, HostCatalogError> {
    transaction.execute_batch(automations::CONVERSATION_DELETIONS)?;
    transaction.execute_batch(automations::RUN_INDEXES)?;
    automations::purge_all_deleted(transaction)
        .map_err(|error| HostCatalogError::Invalid(error.to_string()))
}

/// Turns a current catalog back into the schema 39 shape.
#[cfg(test)]
pub(crate) fn restore_schema_39(connection: &rusqlite::Connection) {
    super::restore_schema_40(connection);
    connection
        .execute_batch("DROP TABLE host_automation_conversation_deletions;")
        .expect("restore the schema 39 automation tables");
}

/// Turns a current catalog back into the schema 38 shape, so upgrade tests
/// start from the tables an earlier runtime wrote.
#[cfg(test)]
pub(crate) fn restore_schema_38(connection: &rusqlite::Connection) {
    restore_schema_39(connection);
    connection
        .execute_batch(
            "DROP INDEX host_automation_runs_by_automation;
             DROP INDEX host_automation_runs_by_agent;",
        )
        .expect("restore the schema 38 run indexes");
}
