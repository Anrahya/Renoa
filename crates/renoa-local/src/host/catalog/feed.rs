//! Schema 46 gives the Host its feed: the items an owner reads under "While
//! you were away", the owner's `seen_through`, and the Today sentence.
//!
//! All three start empty. Nothing is backfilled: runs and activations recorded
//! before the upgrade make no items.

use rusqlite::Transaction;

use super::HostCatalogError;

pub(super) fn add_feed(transaction: &Transaction<'_>) -> Result<(), HostCatalogError> {
    crate::host::feed::initialize(transaction)
}

/// Turns a current catalog back into the schema 45 shape, so upgrade tests
/// start from the tables an earlier runtime wrote.
#[cfg(test)]
pub(crate) fn restore_schema_45(connection: &rusqlite::Connection) {
    connection
        .execute_batch(
            "DROP TABLE host_today;
             DROP TABLE host_feed_state;
             DROP TABLE host_feed_items;",
        )
        .expect("restore the schema 45 tables");
}
