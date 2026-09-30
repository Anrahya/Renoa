//! Schema 35 moved automation runs onto RCP tasks.
//!
//! A run no longer executes in a private Host session of its own, so a run
//! drops its `session_id`, and an automation records the Host session it was
//! created in so its runs return to that conversation. Automations created
//! before schema 35 have no recorded session and run in a conversation of
//! their own.

use rusqlite::Transaction;

use super::HostCatalogError;

pub(super) fn deliver_runs_through_tasks(
    transaction: &Transaction<'_>,
) -> Result<(), HostCatalogError> {
    transaction.execute_batch(
        "ALTER TABLE host_automations ADD COLUMN origin_session_id TEXT;
         ALTER TABLE host_automation_runs DROP COLUMN session_id;",
    )?;
    Ok(())
}

/// Turns a current catalog back into the schema 34 shape, so upgrade tests
/// start from the tables an earlier runtime wrote.
#[cfg(test)]
pub(crate) fn restore_schema_34_automations(connection: &rusqlite::Connection) {
    super::registry_sync::restore_schema_35(connection);
    connection
        .execute_batch(
            "ALTER TABLE host_automations DROP COLUMN origin_session_id;
             ALTER TABLE host_automation_runs
                ADD COLUMN session_id TEXT NOT NULL
                DEFAULT '00000000-0000-0000-0000-000000000000';",
        )
        .expect("restore the schema 34 automation tables");
}
