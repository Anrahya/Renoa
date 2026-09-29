//! Schema 39 bounds automation data. Run history gains the indexes its
//! retention and reads use, and automations deleted before deletion removed
//! their data are purged now, except any with a run still in flight, which its
//! finish purges.

use rusqlite::Transaction;

use super::HostCatalogError;
use crate::host::automations;

pub(super) fn bound_run_history(
    transaction: &Transaction<'_>,
) -> Result<Vec<automations::Removed>, HostCatalogError> {
    transaction.execute_batch(automations::RUN_INDEXES)?;
    automations::purge_all_deleted(transaction)
        .map_err(|error| HostCatalogError::Invalid(error.to_string()))
}

/// Turns a current catalog back into the schema 38 shape, so upgrade tests
/// start from the tables an earlier runtime wrote.
#[cfg(test)]
pub(crate) fn restore_schema_38(connection: &rusqlite::Connection) {
    connection
        .execute_batch(
            "DROP INDEX host_automation_runs_by_automation;
             DROP INDEX host_automation_runs_by_agent;",
        )
        .expect("restore the schema 38 run indexes");
}
