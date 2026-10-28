//! What a task's directory entry reports, from its latest command.
//!
//! A command has settled when its execution stream reached a terminal record;
//! the `command_states` view is the only place that reads that fact. The
//! status class of a terminal is one exhaustive `match`, so a new
//! `ExecutionTerminal` variant fails to compile until it is classified.

use renoa_protocol::{CommandId, ExecutionEventKind, ExecutionTerminal};
use rusqlite::{Connection, OptionalExtension as _};

use crate::{
    ControlError, TaskEventKind, TaskId, TaskStatus,
    store::{id_error, json_error, sqlite_error},
};

impl TaskStatus {
    /// The status once a command's execution has ended in `terminal`.
    pub(crate) const fn after(terminal: &ExecutionTerminal) -> Self {
        match terminal {
            ExecutionTerminal::Completed | ExecutionTerminal::Cancelled { .. } => Self::Idle,
            ExecutionTerminal::Failed { .. } => Self::Failed,
        }
    }
}

/// The status of `task_id`. Only the latest command counts, so an earlier
/// command that never settled cannot hold the task in `Working`.
pub(crate) fn derive(connection: &Connection, task_id: TaskId) -> Result<TaskStatus, ControlError> {
    let latest = connection
        .query_row(
            "SELECT command_id, settled FROM command_states
             WHERE task_id = ?1 ORDER BY ordinal DESC LIMIT 1",
            [task_id.to_string()],
            |row| Ok((row.get::<_, String>(0)?, row.get::<_, bool>(1)?)),
        )
        .optional()
        .map_err(sqlite_error)?;
    let Some((command_id, settled)) = latest else {
        return Ok(TaskStatus::Idle);
    };
    if !settled {
        return Ok(TaskStatus::Working);
    }
    let command_id = CommandId::from_uuid(command_id.parse().map_err(id_error)?);
    Ok(TaskStatus::after(&terminal_of(
        connection, task_id, command_id,
    )?))
}

/// The terminal record of a settled command. It follows the command's other
/// records, so reading the journal from its end finds it first.
fn terminal_of(
    connection: &Connection,
    task_id: TaskId,
    command_id: CommandId,
) -> Result<ExecutionTerminal, ControlError> {
    let mut statement = connection
        .prepare("SELECT kind_json FROM task_events WHERE task_id = ?1 ORDER BY sequence DESC")
        .map_err(sqlite_error)?;
    let rows = statement
        .query_map([task_id.to_string()], |row| row.get::<_, String>(0))
        .map_err(sqlite_error)?;
    for row in rows {
        let kind: TaskEventKind =
            serde_json::from_str(&row.map_err(sqlite_error)?).map_err(json_error)?;
        if let TaskEventKind::ExecutionEvent {
            command_id: cause,
            event,
        } = kind
            && cause == command_id
            && let ExecutionEventKind::ExecutionTerminated { terminal } = event.kind
        {
            return Ok(terminal);
        }
    }
    Err(ControlError::store(format!(
        "settled command {command_id} has no terminal record"
    )))
}
