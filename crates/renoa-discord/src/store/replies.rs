//! Task records become Discord replies: one per finished command, applied
//! exactly once under the task's cursor.

use renoa_control::{TaskEvent, TaskEventKind};
use renoa_protocol::{ExecutionEventKind, ExecutionTerminal};
use rusqlite::{OptionalExtension as _, params};

use super::{SurfaceStore, schema, turns::insert_pages};
use crate::DiscordError;

impl SurfaceStore {
    /// Applies one task record exactly once, advancing the task's cursor in the
    /// same transaction. Returns whether reply pages became ready to post.
    pub(crate) fn apply_event(&self, event: &TaskEvent) -> Result<bool, DiscordError> {
        let event = event.clone();
        self.access(move |connection| {
            let transaction = schema::immediate_transaction(connection)?;
            let task_id = event.task_id.to_string();
            let Some((channel_id, cursor)) = transaction
                .query_row(
                    "SELECT channel_id, cursor FROM tasks WHERE task_id = ?1",
                    [&task_id],
                    |row| Ok((row.get::<_, String>(0)?, row.get::<_, Option<i64>>(1)?)),
                )
                .optional()?
            else {
                return Err(DiscordError::Invalid(format!(
                    "coordinator delivered a record for unknown task {task_id}"
                )));
            };
            let sequence = i64::try_from(event.sequence).map_err(|_| {
                DiscordError::Invalid("task sequence exceeds SQLite range".to_owned())
            })?;
            if cursor.is_some_and(|applied| sequence <= applied) {
                transaction.commit()?;
                return Ok(false);
            }
            let ready = match &event.kind {
                TaskEventKind::CommandSubmitted { command } => {
                    let command_id = command.command_id.to_string();
                    let ours = transaction
                        .query_row(
                            "SELECT 1 FROM turns WHERE command_id = ?1",
                            [&command_id],
                            |_| Ok(()),
                        )
                        .optional()?
                        .is_some();
                    let heading = (!ours).then(|| {
                        format!("**{}:** {}", command.surface.as_str(), command.input.text())
                    });
                    transaction.execute(
                        "INSERT OR IGNORE INTO replies(command_id, task_id, heading)
                         VALUES (?1, ?2, ?3)",
                        params![command_id, task_id, heading],
                    )?;
                    false
                }
                TaskEventKind::ExecutionEvent { command_id, event } => {
                    let command_id = command_id.to_string();
                    transaction.execute(
                        "INSERT OR IGNORE INTO replies(command_id, task_id) VALUES (?1, ?2)",
                        params![command_id, task_id],
                    )?;
                    match &event.kind {
                        ExecutionEventKind::AssistantMessage { text } => {
                            transaction.execute(
                                "UPDATE replies SET answer = ?1 WHERE command_id = ?2",
                                params![text, command_id],
                            )?;
                            false
                        }
                        ExecutionEventKind::ExecutionTerminated { terminal } => {
                            finish_reply(&transaction, &command_id, &channel_id, terminal)?
                        }
                        _ => false,
                    }
                }
            };
            transaction.execute(
                "UPDATE tasks SET cursor = ?1 WHERE task_id = ?2",
                params![sequence, task_id],
            )?;
            transaction.commit()?;
            Ok(ready)
        })
    }
}

/// Turns a finished execution into reply pages. The first page answers the
/// Discord message that caused the command, when this surface sent it.
fn finish_reply(
    connection: &rusqlite::Connection,
    command_id: &str,
    channel_id: &str,
    terminal: &ExecutionTerminal,
) -> Result<bool, DiscordError> {
    let (heading, answer, finished): (Option<String>, Option<String>, bool) = connection
        .query_row(
            "SELECT heading, answer, finished FROM replies WHERE command_id = ?1",
            [command_id],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )?;
    if finished {
        return Ok(false);
    }
    let outcome = match terminal {
        ExecutionTerminal::Completed => answer.unwrap_or_default(),
        ExecutionTerminal::Failed { error } => {
            format!("The agent could not complete this turn: {error}")
        }
        ExecutionTerminal::Cancelled { .. } => "Stopped.".to_owned(),
    };
    let text = match heading {
        Some(heading) => format!("{heading}\n\n{outcome}"),
        None => outcome,
    };
    let reply_to = connection
        .query_row(
            "SELECT message_id FROM turns WHERE command_id = ?1",
            [command_id],
            |row| row.get::<_, String>(0),
        )
        .optional()?;
    connection.execute(
        "UPDATE replies SET finished = 1 WHERE command_id = ?1",
        [command_id],
    )?;
    connection.execute(
        "UPDATE turns SET state = 'answered' WHERE command_id = ?1",
        [command_id],
    )?;
    insert_pages(
        connection,
        command_id,
        channel_id,
        reply_to.as_deref(),
        &text,
    )?;
    Ok(true)
}
