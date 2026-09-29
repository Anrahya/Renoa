//! Schema 38 replaces daily and interval schedules with cron schedules and
//! stores the message each run is sent.
//!
//! A run's submission now carries a context line built at admission, so the
//! column holding it is renamed from `prompt`. Runs admitted earlier were sent
//! their standing task alone, which is exactly what the column holds. Stored
//! daily or interval schedules have no exact cron form, so the upgrade refuses
//! them rather than guess.

use rusqlite::Transaction;

use super::HostCatalogError;

pub(super) fn adopt_cron_schedules(transaction: &Transaction<'_>) -> Result<(), HostCatalogError> {
    let legacy: i64 = transaction.query_row(
        "SELECT (SELECT count(*) FROM host_automations
                 WHERE json_extract(schedule_json, '$.kind') IN ('daily', 'interval'))
              + (SELECT count(*) FROM host_automation_mutations
                 WHERE json_extract(result_json, '$.spec.schedule.kind') IN ('daily', 'interval'))
              + (SELECT count(*) FROM host_automation_owner_mutations
                 WHERE json_extract(result_json, '$.spec.schedule.kind') IN ('daily', 'interval'))",
        [],
        |row| row.get(0),
    )?;
    if legacy > 0 {
        return Err(HostCatalogError::Invalid(format!(
            "{legacy} stored automations or automation receipts use daily or interval schedules, which schema 38 replaces with cron; delete them with the previous release, then upgrade"
        )));
    }
    transaction
        .execute_batch("ALTER TABLE host_automation_runs RENAME COLUMN prompt TO submission;")?;
    Ok(())
}

/// Turns a current catalog back into the schema 37 shape, so upgrade tests
/// start from the tables an earlier runtime wrote.
#[cfg(test)]
pub(crate) fn restore_schema_37(connection: &rusqlite::Connection) {
    connection
        .execute_batch("ALTER TABLE host_automation_runs RENAME COLUMN submission TO prompt;")
        .expect("restore the schema 37 run column");
}
