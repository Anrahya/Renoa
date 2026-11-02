//! Schema 44 gives compiled plugins triggers, and `renoa.memory` the
//! receipts of its nightly jobs and its memory session generations.
//!
//! All three start empty: the scheduler creates the main agent's nightly
//! trigger the first time it looks for due work, and no job has run yet.

use rusqlite::Transaction;

use super::HostCatalogError;
use crate::plugins::host::{
    memory::{jobs, sessions},
    triggers,
};

pub(super) fn add_triggers_and_memory_jobs(
    transaction: &Transaction<'_>,
) -> Result<(), HostCatalogError> {
    triggers::initialize(transaction)?;
    jobs::initialize(transaction)?;
    sessions::initialize(transaction)?;
    Ok(())
}

/// Turns a current catalog back into the schema 43 shape, so upgrade tests
/// start from the tables an earlier runtime wrote.
#[cfg(test)]
pub(crate) fn restore_schema_43(connection: &rusqlite::Connection) {
    connection
        .execute_batch(
            "DROP TABLE host_memory_sessions;
             DROP TABLE host_memory_jobs;
             DELETE FROM host_automation_runs
                WHERE automation_id IN (SELECT automation_id FROM host_plugin_triggers);
             CREATE TEMP TABLE plugin_owned AS SELECT automation_id FROM host_plugin_triggers;
             DROP TABLE host_plugin_triggers;
             DELETE FROM host_automations WHERE id IN (SELECT automation_id FROM plugin_owned);
             DROP TABLE plugin_owned;",
        )
        .expect("restore the schema 43 tables");
}
