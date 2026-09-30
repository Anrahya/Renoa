//! Schema 36 records a failing shared-registry synchronization.
//!
//! The row exists only while this Host's latest synchronization failed, so a
//! process observing the Host, such as the Control Room's, sees the failure
//! and when it began without reading the executing process's log.

use rusqlite::Transaction;

use super::HostCatalogError;

pub(super) fn record_sync_failures(transaction: &Transaction<'_>) -> Result<(), HostCatalogError> {
    transaction.execute_batch(
        "CREATE TABLE shared_plugin_registry_sync (
            singleton INTEGER PRIMARY KEY CHECK (singleton = 1),
            failing_since_ms INTEGER NOT NULL CHECK (failing_since_ms >= 0),
            error TEXT NOT NULL CHECK (length(error) > 0)
        ) STRICT;",
    )?;
    Ok(())
}

/// Turns a current catalog back into the schema 35 shape, so upgrade tests
/// start from the tables an earlier runtime wrote.
#[cfg(test)]
pub(crate) fn restore_schema_35(connection: &rusqlite::Connection) {
    super::automation_outcomes::restore_schema_36(connection);
    connection
        .execute_batch("DROP TABLE shared_plugin_registry_sync;")
        .expect("restore the schema 35 catalog");
}
