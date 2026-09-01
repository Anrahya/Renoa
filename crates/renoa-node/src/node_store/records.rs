use std::{
    path::PathBuf,
    time::{SystemTime, UNIX_EPOCH},
};

use renoa_control::TaskId;
use renoa_protocol::{
    CommandId, ExecutionEvent, ExecutionEventId, ExecutionEventKind, ExecutionId,
};
use rusqlite::{Connection, OptionalExtension, Transaction, params};

use super::{ExecutionRecord, NodeStoreError, TargetBinding, parse_uuid, to_i64};

pub(super) type StoredRow = (
    i64,
    String,
    String,
    String,
    String,
    String,
    String,
    String,
    bool,
    bool,
    Option<i64>,
);

pub(super) fn stored_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<StoredRow> {
    Ok((
        row.get(0)?,
        row.get(1)?,
        row.get(2)?,
        row.get(3)?,
        row.get(4)?,
        row.get(5)?,
        row.get(6)?,
        row.get(7)?,
        row.get(8)?,
        row.get(9)?,
        row.get(10)?,
    ))
}

pub(super) fn decode_record(row: &StoredRow) -> Result<ExecutionRecord, NodeStoreError> {
    Ok(ExecutionRecord {
        admission_sequence: u64::try_from(row.0)
            .map_err(|_| NodeStoreError::Invalid("negative admission sequence".to_owned()))?,
        task_id: TaskId::from_uuid(parse_uuid(&row.1, "task")?),
        command: serde_json::from_str(&row.2)?,
        execution_id: ExecutionId::from_uuid(parse_uuid(&row.3, "execution")?),
        binding: TargetBinding {
            target: row.4.clone(),
            profile_id: row.5.clone(),
            session_id: parse_uuid(&row.6, "session")?,
            workspace: PathBuf::from(&row.7),
        },
        admission_acked: row.8,
        terminal: row.9,
        published_through: row
            .10
            .map(|value| {
                u64::try_from(value)
                    .map_err(|_| NodeStoreError::Invalid("negative publication cursor".to_owned()))
            })
            .transpose()?,
    })
}

pub(super) fn load_record(
    connection: &Connection,
    command_id: CommandId,
) -> Result<Option<ExecutionRecord>, NodeStoreError> {
    let row = connection
        .query_row(
            "SELECT e.admission_sequence, e.task_id, e.command_json, e.execution_id,
                t.target, t.profile_id, t.session_id, t.workspace,
                e.admission_acked, e.terminal, e.published_through
         FROM host_node_executions e
         JOIN host_node_tasks t USING(task_id)
         WHERE e.command_id = ?1",
            [command_id.to_string()],
            stored_row,
        )
        .optional()?;
    row.as_ref().map(decode_record).transpose()
}

pub(super) fn ensure_task_binding(
    transaction: &Transaction<'_>,
    task_id: TaskId,
    binding: &TargetBinding,
) -> Result<(), NodeStoreError> {
    let existing = transaction
        .query_row(
            "SELECT target, profile_id, session_id, workspace
         FROM host_node_tasks WHERE task_id = ?1",
            [task_id.to_string()],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                ))
            },
        )
        .optional()?;
    if let Some((target, profile_id, session_id, workspace)) = existing {
        let existing = TargetBinding {
            target,
            profile_id,
            session_id: parse_uuid(&session_id, "session")?,
            workspace: PathBuf::from(workspace),
        };
        if existing != *binding {
            return Err(NodeStoreError::Invalid(format!(
                "task {task_id} conflicts with its durable Host binding"
            )));
        }
        return Ok(());
    }
    let session_owner = transaction
        .query_row(
            "SELECT task_id FROM host_node_tasks WHERE session_id = ?1",
            [binding.session_id.to_string()],
            |row| row.get::<_, String>(0),
        )
        .optional()?;
    if let Some(owner) = session_owner {
        return Err(NodeStoreError::Invalid(format!(
            "Host session {} is already bound to task {owner}",
            binding.session_id
        )));
    }
    let workspace = binding.workspace.to_str().ok_or_else(|| {
        NodeStoreError::Invalid("Host target workspace is not valid UTF-8".to_owned())
    })?;
    transaction.execute(
        "INSERT INTO host_node_tasks (task_id, target, profile_id, session_id, workspace)
         VALUES (?1, ?2, ?3, ?4, ?5)",
        params![
            task_id.to_string(),
            binding.target,
            binding.profile_id,
            binding.session_id.to_string(),
            workspace,
        ],
    )?;
    Ok(())
}

pub(super) fn insert_event(
    transaction: &Transaction<'_>,
    command_id: CommandId,
    execution_id: ExecutionId,
    sequence: u64,
    kind: ExecutionEventKind,
) -> Result<(), NodeStoreError> {
    let event = ExecutionEvent {
        event_id: ExecutionEventId::new(),
        execution_id,
        sequence,
        recorded_at_ms: now_ms()?,
        kind,
    };
    transaction.execute(
        "INSERT INTO host_node_events (command_id, sequence, event_json)
         VALUES (?1, ?2, ?3)",
        params![
            command_id.to_string(),
            to_i64(sequence, "execution sequence")?,
            serde_json::to_string(&event)?
        ],
    )?;
    Ok(())
}

pub(super) fn load_event(
    transaction: &Transaction<'_>,
    command_id: CommandId,
    sequence: u64,
) -> Result<Option<ExecutionEvent>, NodeStoreError> {
    let json = transaction
        .query_row(
            "SELECT event_json FROM host_node_events
         WHERE command_id = ?1 AND sequence = ?2",
            params![
                command_id.to_string(),
                to_i64(sequence, "execution sequence")?
            ],
            |row| row.get::<_, String>(0),
        )
        .optional()?;
    json.map(|json| serde_json::from_str(&json).map_err(NodeStoreError::from))
        .transpose()
}

pub(super) fn next_event_sequence(
    transaction: &Transaction<'_>,
    command_id: CommandId,
) -> Result<u64, NodeStoreError> {
    let next = transaction.query_row(
        "SELECT COALESCE(MAX(sequence) + 1, 0)
         FROM host_node_events WHERE command_id = ?1",
        [command_id.to_string()],
        |row| row.get::<_, i64>(0),
    )?;
    u64::try_from(next)
        .map_err(|_| NodeStoreError::Invalid("negative execution sequence".to_owned()))
}

fn now_ms() -> Result<i64, NodeStoreError> {
    let milliseconds = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|_| NodeStoreError::Clock)?
        .as_millis();
    i64::try_from(milliseconds)
        .map_err(|_| NodeStoreError::Invalid("node timestamp exceeds i64 range".to_owned()))
}
