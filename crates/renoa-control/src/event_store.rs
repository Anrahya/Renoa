use std::sync::Arc;

use renoa_protocol::{CommandId, ExecutionEvent, ExecutionEventKind, ExecutionId, PrincipalId};
use rusqlite::{OptionalExtension, Transaction, TransactionBehavior, params};

use crate::{
    ControlError, TaskEvent, TaskEventId, TaskEventKind, TaskId,
    control_schema::open_connection,
    store::{
        ControlStore, blocking, id_error, json_error, sqlite_error, task_not_found, to_sql_integer,
    },
};

#[derive(Debug)]
pub(crate) struct TaskSuffix {
    pub(crate) through_sequence: Option<u64>,
    pub(crate) events: Vec<TaskEvent>,
}

#[derive(Debug)]
pub(crate) struct ExecutionEventAdmission {
    pub(crate) through_execution_sequence: u64,
    pub(crate) events: Vec<TaskEvent>,
}

impl ControlStore {
    pub(crate) async fn append_execution_events(
        &self,
        task_id: TaskId,
        command_id: CommandId,
        events: Vec<ExecutionEvent>,
    ) -> Result<ExecutionEventAdmission, ControlError> {
        let path = Arc::clone(&self.path);
        blocking(move || {
            let mut connection = open_connection(&path)?;
            let transaction = connection
                .transaction_with_behavior(TransactionBehavior::Immediate)
                .map_err(sqlite_error)?;
            let admission =
                admit_execution_event_batch(&transaction, task_id, command_id, &events)?;
            transaction.commit().map_err(sqlite_error)?;
            Ok(admission)
        })
        .await
    }

    pub(crate) async fn load_suffix(
        &self,
        task_id: TaskId,
        principal_id: PrincipalId,
        after_sequence: Option<u64>,
    ) -> Result<TaskSuffix, ControlError> {
        let path = Arc::clone(&self.path);
        blocking(move || {
            let mut connection = open_connection(&path)?;
            let transaction = connection
                .transaction_with_behavior(TransactionBehavior::Deferred)
                .map_err(sqlite_error)?;
            let next_sequence = transaction
                .query_row(
                    "SELECT next_sequence FROM tasks
                     WHERE task_id = ?1 AND principal_id = ?2",
                    params![task_id.to_string(), principal_id.to_string()],
                    |row| row.get::<_, i64>(0),
                )
                .optional()
                .map_err(sqlite_error)?
                .ok_or_else(|| task_not_found(task_id))?;
            let next_sequence = u64::try_from(next_sequence)
                .map_err(|_| ControlError::store("negative task sequence"))?;
            let through_sequence = next_sequence.checked_sub(1);
            if let Some(cursor) = after_sequence
                && through_sequence.is_none_or(|through| cursor > through)
            {
                return Err(ControlError::invalid(format!(
                    "task cursor {cursor} is ahead of durable task history"
                )));
            }
            let minimum = after_sequence.map_or(0, |sequence| sequence.saturating_add(1));
            let minimum = to_sql_integer(minimum)?;
            let mut statement = transaction
                .prepare(
                    "SELECT event_id, sequence, kind_json
                     FROM task_events
                     WHERE task_id = ?1 AND sequence >= ?2
                     ORDER BY sequence",
                )
                .map_err(sqlite_error)?;
            let rows = statement
                .query_map(params![task_id.to_string(), minimum], |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, i64>(1)?,
                        row.get::<_, String>(2)?,
                    ))
                })
                .map_err(sqlite_error)?;
            let mut events = Vec::new();
            for row in rows {
                let (event_id, sequence, kind_json) = row.map_err(sqlite_error)?;
                events.push(TaskEvent {
                    event_id: TaskEventId::from_uuid(event_id.parse().map_err(id_error)?),
                    task_id,
                    sequence: u64::try_from(sequence)
                        .map_err(|_| ControlError::store("negative task event sequence"))?,
                    kind: serde_json::from_str(&kind_json).map_err(json_error)?,
                });
            }
            drop(statement);
            transaction.commit().map_err(sqlite_error)?;
            Ok(TaskSuffix {
                through_sequence,
                events,
            })
        })
        .await
    }
}

fn admit_execution_event_batch(
    transaction: &Transaction<'_>,
    task_id: TaskId,
    command_id: CommandId,
    events: &[ExecutionEvent],
) -> Result<ExecutionEventAdmission, ControlError> {
    ensure_admitted_command(transaction, task_id, command_id)?;
    let Some(first) = events.first() else {
        return Err(ControlError::invalid("execution event batch is empty"));
    };
    let stream = load_execution_event_stream(transaction, command_id)?;
    let mut next_sequence = initial_execution_event_cursor(stream, first, command_id)?;
    validate_execution_event_batch(events)?;
    if first.sequence > next_sequence {
        return Err(ControlError::invalid(format!(
            "execution event batch starts at sequence {}; expected at most {next_sequence}",
            first.sequence
        )));
    }
    let mut terminal = stream.is_some_and(|stream| stream.terminal);
    let appended = append_contiguous_execution_events(
        transaction,
        task_id,
        command_id,
        events,
        &mut next_sequence,
        &mut terminal,
    )?;
    store_execution_event_cursor(
        transaction,
        command_id,
        first.execution_id,
        stream.is_some(),
        next_sequence,
        terminal,
    )?;
    Ok(ExecutionEventAdmission {
        through_execution_sequence: next_sequence
            .checked_sub(1)
            .ok_or_else(|| ControlError::store("execution event cursor did not advance"))?,
        events: appended,
    })
}

fn initial_execution_event_cursor(
    stream: Option<StoredExecutionEventStream>,
    first: &ExecutionEvent,
    command_id: CommandId,
) -> Result<u64, ControlError> {
    match stream {
        Some(stream) if stream.execution_id == first.execution_id => Ok(stream.next_sequence),
        Some(stream) => Err(ControlError::conflict(format!(
            "command {command_id} is already bound to execution {}",
            stream.execution_id
        ))),
        None if first.sequence == 0 => Ok(0),
        None => Err(ControlError::invalid(
            "execution event stream must start at sequence zero",
        )),
    }
}

fn append_contiguous_execution_events(
    transaction: &Transaction<'_>,
    task_id: TaskId,
    command_id: CommandId,
    events: &[ExecutionEvent],
    next_sequence: &mut u64,
    terminal: &mut bool,
) -> Result<Vec<TaskEvent>, ControlError> {
    let mut appended = Vec::with_capacity(events.len());
    for event in events {
        let source_id = format!("execution:{}", event.event_id);
        let kind = TaskEventKind::ExecutionEvent {
            command_id,
            event: event.clone(),
        };
        let existing = load_source_event(transaction, &source_id)?;
        validate_existing_source(existing.as_ref(), task_id, &kind, &source_id)?;
        if event.sequence < *next_sequence {
            if existing.is_none() {
                return Err(ControlError::conflict(format!(
                    "execution sequence {} was already accepted with another event identity",
                    event.sequence
                )));
            }
            continue;
        }
        if event.sequence > *next_sequence {
            return Err(ControlError::invalid(format!(
                "execution event sequence {} leaves a gap after {next_sequence}",
                event.sequence
            )));
        }
        if *terminal {
            return Err(ControlError::invalid(format!(
                "execution {} is already terminal",
                event.execution_id
            )));
        }
        if existing.is_none() {
            appended.push(append_event(transaction, task_id, &source_id, &kind)?);
        }
        *next_sequence = next_sequence
            .checked_add(1)
            .ok_or_else(|| ControlError::invalid("execution sequence overflow"))?;
        *terminal = matches!(&event.kind, ExecutionEventKind::ExecutionTerminated { .. });
    }
    Ok(appended)
}

fn validate_existing_source(
    existing: Option<&TaskEvent>,
    task_id: TaskId,
    kind: &TaskEventKind,
    source_id: &str,
) -> Result<(), ControlError> {
    if existing.is_some_and(|event| event.task_id != task_id || event.kind != *kind) {
        return Err(ControlError::conflict(format!(
            "event source {source_id} was already used with different content"
        )));
    }
    Ok(())
}

fn store_execution_event_cursor(
    transaction: &Transaction<'_>,
    command_id: CommandId,
    execution_id: ExecutionId,
    exists: bool,
    next_sequence: u64,
    terminal: bool,
) -> Result<(), ControlError> {
    let next_sequence = to_sql_integer(next_sequence)?;
    if exists {
        transaction
            .execute(
                "UPDATE execution_event_streams
                 SET next_sequence = ?2, terminal = ?3
                 WHERE command_id = ?1",
                params![command_id.to_string(), next_sequence, terminal],
            )
            .map_err(sqlite_error)?;
    } else {
        transaction
            .execute(
                "INSERT INTO execution_event_streams (
                    command_id, execution_id, next_sequence, terminal
                 ) VALUES (?1, ?2, ?3, ?4)",
                params![
                    command_id.to_string(),
                    execution_id.to_string(),
                    next_sequence,
                    terminal,
                ],
            )
            .map_err(sqlite_error)?;
    }
    Ok(())
}

#[derive(Debug, Clone, Copy)]
struct StoredExecutionEventStream {
    execution_id: ExecutionId,
    next_sequence: u64,
    terminal: bool,
}

fn load_execution_event_stream(
    transaction: &Transaction<'_>,
    command_id: CommandId,
) -> Result<Option<StoredExecutionEventStream>, ControlError> {
    let row = transaction
        .query_row(
            "SELECT execution_id, next_sequence, terminal
             FROM execution_event_streams WHERE command_id = ?1",
            [command_id.to_string()],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, i64>(1)?,
                    row.get::<_, bool>(2)?,
                ))
            },
        )
        .optional()
        .map_err(sqlite_error)?;
    row.map(|(execution_id, next_sequence, terminal)| {
        Ok(StoredExecutionEventStream {
            execution_id: ExecutionId::from_uuid(execution_id.parse().map_err(id_error)?),
            next_sequence: u64::try_from(next_sequence)
                .map_err(|_| ControlError::store("negative execution event sequence"))?,
            terminal,
        })
    })
    .transpose()
}

fn ensure_admitted_command(
    transaction: &Transaction<'_>,
    task_id: TaskId,
    command_id: CommandId,
) -> Result<(), ControlError> {
    let admitted = transaction
        .query_row(
            "SELECT EXISTS(
                SELECT 1 FROM commands WHERE command_id = ?1 AND task_id = ?2
             )",
            params![command_id.to_string(), task_id.to_string()],
            |row| row.get::<_, bool>(0),
        )
        .map_err(sqlite_error)?;
    if !admitted {
        return Err(ControlError::not_found(format!(
            "command {command_id} was not admitted for task {task_id}"
        )));
    }
    Ok(())
}

fn validate_execution_event_batch(events: &[ExecutionEvent]) -> Result<(), ControlError> {
    let first = events.first().expect("batch is non-empty");
    let execution_id = first.execution_id;
    for (offset, event) in events.iter().enumerate() {
        let final_event = offset + 1 == events.len();
        let offset = u64::try_from(offset)
            .map_err(|_| ControlError::invalid("execution event batch is too large"))?;
        let sequence = first
            .sequence
            .checked_add(offset)
            .ok_or_else(|| ControlError::invalid("execution sequence overflow"))?;
        if event.execution_id != execution_id || event.sequence != sequence {
            return Err(ControlError::invalid(
                "execution event batch must continue one contiguous execution",
            ));
        }
        match &event.kind {
            ExecutionEventKind::ExecutionStarted if sequence == 0 => {}
            ExecutionEventKind::ExecutionStarted => {
                return Err(ControlError::invalid(
                    "execution_started must be sequence zero",
                ));
            }
            ExecutionEventKind::ExecutionTerminated { .. } if !final_event => {
                return Err(ControlError::invalid(
                    "execution_terminated must be the final event",
                ));
            }
            _ if sequence == 0 => {
                return Err(ControlError::invalid(
                    "execution event stream must start with execution_started",
                ));
            }
            _ => {}
        }
    }
    Ok(())
}

fn load_source_event(
    transaction: &Transaction<'_>,
    source_id: &str,
) -> Result<Option<TaskEvent>, ControlError> {
    let row = transaction
        .query_row(
            "SELECT event_id, task_id, sequence, kind_json
             FROM task_events WHERE source_id = ?1",
            [source_id],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, i64>(2)?,
                    row.get::<_, String>(3)?,
                ))
            },
        )
        .optional()
        .map_err(sqlite_error)?;
    row.map(|(event_id, task_id, sequence, kind_json)| {
        Ok(TaskEvent {
            event_id: TaskEventId::from_uuid(event_id.parse().map_err(id_error)?),
            task_id: TaskId::from_uuid(task_id.parse().map_err(id_error)?),
            sequence: u64::try_from(sequence)
                .map_err(|_| ControlError::store("negative task event sequence"))?,
            kind: serde_json::from_str(&kind_json).map_err(json_error)?,
        })
    })
    .transpose()
}

pub(crate) fn append_event(
    transaction: &Transaction<'_>,
    task_id: TaskId,
    source_id: &str,
    kind: &TaskEventKind,
) -> Result<TaskEvent, ControlError> {
    let sequence = transaction
        .query_row(
            "SELECT next_sequence FROM tasks WHERE task_id = ?1",
            [task_id.to_string()],
            |row| row.get::<_, i64>(0),
        )
        .map_err(sqlite_error)?;
    let sequence =
        u64::try_from(sequence).map_err(|_| ControlError::store("negative task sequence"))?;
    let event = TaskEvent {
        event_id: TaskEventId::new(),
        task_id,
        sequence,
        kind: kind.clone(),
    };
    transaction
        .execute(
            "INSERT INTO task_events (
                event_id, task_id, sequence, source_id, kind_json
             ) VALUES (?1, ?2, ?3, ?4, ?5)",
            params![
                event.event_id.to_string(),
                task_id.to_string(),
                to_sql_integer(sequence)?,
                source_id,
                serde_json::to_string(kind).map_err(json_error)?,
            ],
        )
        .map_err(sqlite_error)?;
    transaction
        .execute(
            "UPDATE tasks SET next_sequence = ?2 WHERE task_id = ?1",
            params![task_id.to_string(), to_sql_integer(sequence + 1)?],
        )
        .map_err(sqlite_error)?;
    Ok(event)
}
