//! Schema 42 records the Host's main agent.
//!
//! The marker and its owner receipts are created, and the agent an earlier
//! runtime made first is marked: the earliest `created_at_ms`, ties broken by
//! agent id. A Host with no agent is left unmarked until its first creation.

use rusqlite::Transaction;

use super::HostCatalogError;
use crate::host::definition::main_agent;

pub(super) fn mark_main_agent(transaction: &Transaction<'_>) -> Result<(), HostCatalogError> {
    main_agent::initialize(transaction)?;
    main_agent::mark_first_if_unmarked(transaction)
}

/// Turns a current catalog back into the schema 41 shape, so upgrade tests
/// start from the tables an earlier runtime wrote.
#[cfg(test)]
pub(crate) fn restore_schema_41(connection: &rusqlite::Connection) {
    connection
        .execute_batch(
            "DROP TABLE host_main_agent_operations;
             DROP TABLE host_main_agent;",
        )
        .expect("restore the schema 41 agent tables");
}
