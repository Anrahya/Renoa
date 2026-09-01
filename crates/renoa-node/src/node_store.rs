use std::{
    path::{Path, PathBuf},
    sync::Arc,
};

use renoa_control::TaskId;
use renoa_protocol::{CommandEnvelope, CommandId, ExecutionEvent, ExecutionEventKind, ExecutionId};
use rusqlite::{TransactionBehavior, params};
use thiserror::Error;
use uuid::Uuid;

mod records;
mod schema;

use records::{
    decode_record, ensure_task_binding, insert_event, load_event, load_record, next_event_sequence,
    stored_row,
};
use schema::{initialize, open_connection};

#[derive(Debug, Error)]
pub(crate) enum NodeStoreError {
    #[error("node SQLite failed: {0}")]
    Sqlite(#[from] rusqlite::Error),
    #[error("node ledger JSON is invalid: {0}")]
    Json(#[from] serde_json::Error),
    #[error("node ledger is invalid: {0}")]
    Invalid(String),
    #[error("node SQLite worker failed: {0}")]
    Worker(#[from] tokio::task::JoinError),
    #[error("node clock is before the Unix epoch")]
    Clock,
    #[error("node ledger permissions could not be restricted: {0}")]
    Permissions(#[from] std::io::Error),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct TargetBinding {
    pub(crate) target: String,
    pub(crate) profile_id: String,
    pub(crate) session_id: Uuid,
    pub(crate) workspace: PathBuf,
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct ExecutionRecord {
    pub(crate) admission_sequence: u64,
    pub(crate) task_id: TaskId,
    pub(crate) command: CommandEnvelope,
    pub(crate) execution_id: ExecutionId,
    pub(crate) binding: TargetBinding,
    pub(crate) admission_acked: bool,
    pub(crate) terminal: bool,
    pub(crate) published_through: Option<u64>,
}

#[derive(Debug, Clone)]
pub(crate) struct NodeStore {
    path: Arc<PathBuf>,
}

impl NodeStore {
    pub(crate) fn open(path: impl AsRef<Path>) -> Result<Self, NodeStoreError> {
        let path = path.as_ref().to_path_buf();
        initialize(&path)?;
        Ok(Self {
            path: Arc::new(path),
        })
    }

    pub(crate) fn validate_configured_targets(
        &self,
        targets: &[TargetBinding],
    ) -> Result<(), NodeStoreError> {
        let connection = open_connection(&self.path)?;
        let mut statement = connection.prepare(
            "SELECT target, profile_id, session_id, workspace
             FROM host_node_tasks ORDER BY task_id",
        )?;
        let rows = statement.query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
            ))
        })?;
        for row in rows {
            let (target, profile_id, session_id, workspace) = row?;
            let stored = TargetBinding {
                target: target.clone(),
                profile_id,
                session_id: parse_uuid(&session_id, "session")?,
                workspace: PathBuf::from(workspace),
            };
            if !targets.iter().any(|configured| configured == &stored) {
                return Err(NodeStoreError::Invalid(format!(
                    "durable task target `{target}` does not match current node configuration"
                )));
            }
        }
        Ok(())
    }

    pub(crate) async fn admit(
        &self,
        task_id: TaskId,
        command: CommandEnvelope,
        binding: TargetBinding,
    ) -> Result<ExecutionRecord, NodeStoreError> {
        let path = Arc::clone(&self.path);
        blocking(move || {
            let mut connection = open_connection(&path)?;
            let transaction =
                connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
            if command.target.as_str() != binding.target {
                return Err(NodeStoreError::Invalid(format!(
                    "command target `{}` does not match configured target `{}`",
                    command.target.as_str(),
                    binding.target
                )));
            }
            ensure_task_binding(&transaction, task_id, &binding)?;
            if let Some(existing) = load_record(&transaction, command.command_id)? {
                if existing.task_id != task_id
                    || existing.command != command
                    || existing.binding != binding
                {
                    return Err(NodeStoreError::Invalid(format!(
                        "command {} conflicts with its durable node admission",
                        command.command_id
                    )));
                }
                transaction.commit()?;
                return Ok(existing);
            }

            let command_json = serde_json::to_string(&command)?;
            let execution_id = ExecutionId::from_uuid(command.command_id.as_uuid());
            transaction.execute(
                "INSERT INTO host_node_executions (
                    command_id, task_id, command_json, execution_id,
                    admission_acked, terminal, published_through
                 ) VALUES (?1, ?2, ?3, ?4, 0, 0, NULL)",
                params![
                    command.command_id.to_string(),
                    task_id.to_string(),
                    command_json,
                    execution_id.to_string(),
                ],
            )?;
            let admission_sequence =
                u64::try_from(transaction.last_insert_rowid()).map_err(|_| {
                    NodeStoreError::Invalid("negative execution admission sequence".to_owned())
                })?;
            insert_event(
                &transaction,
                command.command_id,
                execution_id,
                0,
                ExecutionEventKind::ExecutionStarted,
            )?;
            transaction.commit()?;
            Ok(ExecutionRecord {
                admission_sequence,
                task_id,
                command,
                execution_id,
                binding,
                admission_acked: false,
                terminal: false,
                published_through: None,
            })
        })
        .await
    }

    pub(crate) async fn load_unfinished(&self) -> Result<Vec<ExecutionRecord>, NodeStoreError> {
        let path = Arc::clone(&self.path);
        blocking(move || {
            let connection = open_connection(&path)?;
            let mut statement = connection.prepare(
                "SELECT e.admission_sequence, e.task_id, e.command_json, e.execution_id,
                        t.target, t.profile_id, t.session_id, t.workspace,
                        e.admission_acked, e.terminal, e.published_through
                 FROM host_node_executions e
                 JOIN host_node_tasks t USING(task_id)
                 WHERE e.terminal = 0
                 ORDER BY e.admission_sequence",
            )?;
            let rows = statement.query_map([], stored_row)?;
            rows.map(|row| decode_record(&row?)).collect()
        })
        .await
    }

    pub(crate) async fn load_pending_publications(
        &self,
    ) -> Result<Vec<ExecutionRecord>, NodeStoreError> {
        let path = Arc::clone(&self.path);
        blocking(move || {
            let connection = open_connection(&path)?;
            let mut statement = connection.prepare(
                "SELECT e.admission_sequence, e.task_id, e.command_json, e.execution_id,
                        t.target, t.profile_id, t.session_id, t.workspace,
                        e.admission_acked, e.terminal, e.published_through
                 FROM host_node_executions e
                 JOIN host_node_tasks t USING(task_id)
                 WHERE e.terminal = 0
                    OR e.admission_acked = 0
                    OR e.published_through IS NULL
                    OR e.published_through < (
                        SELECT MAX(sequence) FROM host_node_events
                        WHERE command_id = e.command_id
                    )
                 ORDER BY e.admission_sequence",
            )?;
            let rows = statement.query_map([], stored_row)?;
            rows.map(|row| decode_record(&row?)).collect()
        })
        .await
    }

    pub(crate) async fn append_turn_started(
        &self,
        command_id: CommandId,
    ) -> Result<(), NodeStoreError> {
        let path = Arc::clone(&self.path);
        blocking(move || {
            let mut connection = open_connection(&path)?;
            let transaction =
                connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
            let record = load_record(&transaction, command_id)?.ok_or_else(|| {
                NodeStoreError::Invalid(format!("execution for command {command_id} was not found"))
            })?;
            if record.terminal {
                return Err(NodeStoreError::Invalid(format!(
                    "execution for command {command_id} is already terminal"
                )));
            }
            let existing = load_event(&transaction, command_id, 1)?;
            if let Some(existing) = existing {
                if !matches!(existing.kind, ExecutionEventKind::TurnStarted) {
                    return Err(NodeStoreError::Invalid(format!(
                        "execution for command {command_id} has invalid sequence one"
                    )));
                }
            } else {
                insert_event(
                    &transaction,
                    command_id,
                    record.execution_id,
                    1,
                    ExecutionEventKind::TurnStarted,
                )?;
            }
            transaction.commit()?;
            Ok(())
        })
        .await
    }

    pub(crate) async fn finish(
        &self,
        command_id: CommandId,
        kinds: Vec<ExecutionEventKind>,
    ) -> Result<(), NodeStoreError> {
        let path = Arc::clone(&self.path);
        blocking(move || {
            let mut connection = open_connection(&path)?;
            let transaction =
                connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
            let record = load_record(&transaction, command_id)?.ok_or_else(|| {
                NodeStoreError::Invalid(format!("execution for command {command_id} was not found"))
            })?;
            if record.terminal {
                transaction.commit()?;
                return Ok(());
            }
            let next = next_event_sequence(&transaction, command_id)?;
            let terminal_count = kinds
                .iter()
                .filter(|kind| matches!(kind, ExecutionEventKind::ExecutionTerminated { .. }))
                .count();
            if terminal_count != 1
                || !matches!(
                    kinds.last(),
                    Some(ExecutionEventKind::ExecutionTerminated { .. })
                )
            {
                return Err(NodeStoreError::Invalid(
                    "completed execution projection must end with one terminal event".to_owned(),
                ));
            }
            for (offset, kind) in kinds.into_iter().enumerate() {
                let offset = u64::try_from(offset)
                    .map_err(|_| NodeStoreError::Invalid("too many execution events".to_owned()))?;
                let sequence = next.checked_add(offset).ok_or_else(|| {
                    NodeStoreError::Invalid("execution sequence overflow".to_owned())
                })?;
                insert_event(
                    &transaction,
                    command_id,
                    record.execution_id,
                    sequence,
                    kind,
                )?;
            }
            transaction.execute(
                "UPDATE host_node_executions SET terminal = 1 WHERE command_id = ?1",
                [command_id.to_string()],
            )?;
            transaction.commit()?;
            Ok(())
        })
        .await
    }

    pub(crate) async fn load_events_after(
        &self,
        command_id: CommandId,
        after: Option<u64>,
    ) -> Result<Vec<ExecutionEvent>, NodeStoreError> {
        let path = Arc::clone(&self.path);
        blocking(move || {
            let connection = open_connection(&path)?;
            let minimum = after.map_or(0, |value| value.saturating_add(1));
            let minimum = to_i64(minimum, "execution event cursor")?;
            let mut statement = connection.prepare(
                "SELECT event_json FROM host_node_events
                 WHERE command_id = ?1 AND sequence >= ?2 ORDER BY sequence",
            )?;
            let rows = statement.query_map(params![command_id.to_string(), minimum], |row| {
                row.get::<_, String>(0)
            })?;
            rows.map(|row| Ok(serde_json::from_str(&row?)?)).collect()
        })
        .await
    }

    pub(crate) async fn require_admission_ack(
        &self,
        command_id: CommandId,
    ) -> Result<(), NodeStoreError> {
        self.set_admission_ack(command_id, false).await
    }

    pub(crate) async fn acknowledge_admission(
        &self,
        command_id: CommandId,
    ) -> Result<(), NodeStoreError> {
        self.set_admission_ack(command_id, true).await
    }

    pub(crate) async fn advance_publication(
        &self,
        command_id: CommandId,
        through_sequence: u64,
    ) -> Result<(), NodeStoreError> {
        let path = Arc::clone(&self.path);
        blocking(move || {
            let connection = open_connection(&path)?;
            let through_sequence = to_i64(through_sequence, "publication cursor")?;
            let changed = connection.execute(
                "UPDATE host_node_executions
                 SET published_through = CASE
                    WHEN published_through IS NULL OR published_through < ?2 THEN ?2
                    ELSE published_through
                 END
                 WHERE command_id = ?1",
                params![command_id.to_string(), through_sequence],
            )?;
            require_one(changed, command_id)
        })
        .await
    }

    async fn set_admission_ack(
        &self,
        command_id: CommandId,
        acknowledged: bool,
    ) -> Result<(), NodeStoreError> {
        let path = Arc::clone(&self.path);
        blocking(move || {
            let connection = open_connection(&path)?;
            let changed = connection.execute(
                "UPDATE host_node_executions SET admission_acked = ?2 WHERE command_id = ?1",
                params![command_id.to_string(), acknowledged],
            )?;
            require_one(changed, command_id)
        })
        .await
    }
}

async fn blocking<T>(
    operation: impl FnOnce() -> Result<T, NodeStoreError> + Send + 'static,
) -> Result<T, NodeStoreError>
where
    T: Send + 'static,
{
    tokio::task::spawn_blocking(operation).await?
}

fn require_one(changed: usize, command_id: CommandId) -> Result<(), NodeStoreError> {
    if changed == 1 {
        Ok(())
    } else {
        Err(NodeStoreError::Invalid(format!(
            "execution for command {command_id} was not found"
        )))
    }
}

pub(super) fn parse_uuid(value: &str, name: &str) -> Result<Uuid, NodeStoreError> {
    value
        .parse()
        .map_err(|error| NodeStoreError::Invalid(format!("invalid {name} id: {error}")))
}

pub(super) fn to_i64(value: u64, name: &str) -> Result<i64, NodeStoreError> {
    i64::try_from(value)
        .map_err(|_| NodeStoreError::Invalid(format!("{name} exceeds SQLite i64 range")))
}
