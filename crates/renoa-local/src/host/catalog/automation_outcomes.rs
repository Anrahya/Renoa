//! Schema 37 gives every finished automation run a status.
//!
//! A run records whether it succeeded, failed, or was skipped, how many of its
//! tool calls failed, and when it finished, and the scheduler records a
//! heartbeat. Runs finished before schema 37 carry only their result text, so
//! the upgrade derives their status from the failure texts earlier schedulers
//! wrote; their tool failures and finishing time stay unknown.

use rusqlite::Transaction;

use super::HostCatalogError;

pub(super) fn record_run_outcomes(transaction: &Transaction<'_>) -> Result<(), HostCatalogError> {
    transaction.execute_batch(
        "ALTER TABLE host_automation_runs
            ADD COLUMN status TEXT CHECK(status IN ('succeeded','failed','skipped'));
         ALTER TABLE host_automation_runs
            ADD COLUMN failed_tool_calls INTEGER CHECK(failed_tool_calls >= 0);
         ALTER TABLE host_automation_runs ADD COLUMN finished_at_ms INTEGER;
         UPDATE host_automation_runs
         SET status = CASE
             WHEN output LIKE 'Scheduled run failed:%'
               OR output LIKE 'Scheduled run stopped%'
               OR output LIKE 'Scheduled run could not be sent:%'
               OR output LIKE 'Scheduled run needs input.%' THEN 'failed'
             ELSE 'succeeded' END
         WHERE output IS NOT NULL;",
    )?;
    transaction.execute_batch(crate::host::automations::SCHEDULER_TABLE)?;
    Ok(())
}

/// Turns a current catalog back into the schema 36 shape, so upgrade tests
/// start from the tables an earlier runtime wrote.
#[cfg(test)]
pub(crate) fn restore_schema_36(connection: &rusqlite::Connection) {
    connection
        .execute_batch(
            "ALTER TABLE host_automation_runs DROP COLUMN status;
             ALTER TABLE host_automation_runs DROP COLUMN failed_tool_calls;
             ALTER TABLE host_automation_runs DROP COLUMN finished_at_ms;
             DROP TABLE host_automation_scheduler;",
        )
        .expect("restore the schema 36 automation tables");
}
