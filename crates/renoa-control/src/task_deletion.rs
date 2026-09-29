//! The owning principal deletes a task: its record, journal, commands and
//! execution state go, and nothing is archived.
//!
//! Deletion is idempotent on the task identity: deleting a task that no longer
//! exists succeeds, so a retry after a lost reply converges. A task with a
//! command whose execution has not terminated is refused and left intact, and
//! another principal's task reads as not found.

use std::sync::Arc;

use renoa_protocol::PrincipalId;
use rusqlite::{OptionalExtension as _, TransactionBehavior};

use crate::{
    ControlError, TaskId, control_log,
    control_schema::open_connection,
    coordinator::CoordinatorState,
    store::{blocking, sqlite_error, task_not_found},
};

/// Deletes `task_id` for `principal_id`; a task that no longer exists is
/// already deleted.
pub(crate) async fn delete_task(
    state: &CoordinatorState,
    principal_id: PrincipalId,
    task_id: TaskId,
) -> Result<(), ControlError> {
    let path = Arc::clone(&state.store.path);
    let deleted = blocking(move || {
        let mut connection = open_connection(&path)?;
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(sqlite_error)?;
        let task = task_id.to_string();
        let owner = transaction
            .query_row(
                "SELECT principal_id FROM tasks WHERE task_id = ?1",
                [&task],
                |row| row.get::<_, String>(0),
            )
            .optional()
            .map_err(sqlite_error)?;
        let Some(owner) = owner else {
            return Ok(None);
        };
        if owner != principal_id.to_string() {
            return Err(task_not_found(task_id));
        }
        let in_flight: bool = transaction
            .query_row(
                // A command is settled only by a terminal execution: before its
                // first event is accepted, an acknowledged execution has no
                // stream yet but may be running.
                "SELECT EXISTS(SELECT 1 FROM commands c WHERE c.task_id = ?1
                    AND NOT EXISTS(SELECT 1 FROM execution_event_streams s
                                   WHERE s.command_id = c.command_id AND s.terminal = 1))",
                [&task],
                |row| row.get(0),
            )
            .map_err(sqlite_error)?;
        if in_flight {
            return Err(ControlError::conflict(format!(
                "task {task_id} has an execution in flight; delete it once that finishes"
            )));
        }
        let removed = remove(&transaction, &task).map_err(sqlite_error)?;
        transaction.commit().map_err(sqlite_error)?;
        Ok(Some(removed))
    })
    .await?;
    let Some((events, commands)) = deleted else {
        return Ok(());
    };
    state.task_senders.lock().await.remove(&task_id);
    control_log::event(
        "info",
        "task_deleted",
        &serde_json::json!({
            "task_id": task_id,
            "principal_id": principal_id,
            "events": events,
            "commands": commands,
        }),
    );
    Ok(())
}

/// Deletes a task's rows child first, returning its journal entries and
/// commands. A task with an execution in flight never reaches it.
fn remove(transaction: &rusqlite::Transaction<'_>, task: &str) -> rusqlite::Result<(usize, usize)> {
    transaction.execute(
        "DELETE FROM execution_event_streams WHERE command_id IN
            (SELECT command_id FROM commands WHERE task_id = ?1)",
        [task],
    )?;
    let events = transaction.execute("DELETE FROM task_events WHERE task_id = ?1", [task])?;
    let commands = transaction.execute("DELETE FROM commands WHERE task_id = ?1", [task])?;
    transaction.execute("DELETE FROM tasks WHERE task_id = ?1", [task])?;
    Ok((events, commands))
}
