use std::{
    path::{Path, PathBuf},
    sync::Arc,
};

use renoa_protocol::{
    CommandEnvelope, CommandId, CommandInput, PrincipalId, SurfaceRef, TargetRef,
};
use rusqlite::{Connection, OptionalExtension, TransactionBehavior, params};

use crate::{
    ControlError, NodeId, TaskEvent, TaskEventKind, TaskId, TaskSpec, TaskSummary,
    control_schema::{initialize, open_connection},
    event_store::append_event,
};

#[derive(Debug, Clone)]
pub(crate) struct ControlStore {
    pub(crate) path: Arc<PathBuf>,
}

#[derive(Debug, Clone)]
pub(crate) struct StoredTask {
    pub(crate) principal_id: PrincipalId,
    pub(crate) node_id: NodeId,
    pub(crate) target: TargetRef,
}

#[derive(Debug)]
pub(crate) enum CommandAdmission {
    NotAdmitted,
    Admitted {
        command: CommandEnvelope,
        event: Box<TaskEvent>,
    },
    Existing {
        command: CommandEnvelope,
        pending: bool,
    },
}

impl ControlStore {
    pub(crate) fn open(path: impl AsRef<Path>) -> Result<Self, ControlError> {
        let path = path.as_ref().to_path_buf();
        let mut connection = open_connection(&path)?;
        initialize(&mut connection)?;
        Ok(Self {
            path: Arc::new(path),
        })
    }

    pub(crate) async fn create_task(&self, task: TaskSpec) -> Result<(), ControlError> {
        let path = Arc::clone(&self.path);
        blocking(move || {
            let connection = open_connection(&path)?;
            let target_json = serde_json::to_string(&task.target).map_err(json_error)?;
            connection
                .execute(
                    "INSERT INTO tasks (
                        task_id, principal_id, node_id, target_json, next_sequence
                     ) VALUES (?1, ?2, ?3, ?4, 0)",
                    params![
                        task.task_id.to_string(),
                        task.principal_id.to_string(),
                        task.node_id.to_string(),
                        target_json,
                    ],
                )
                .map_err(sqlite_error)?;
            Ok(())
        })
        .await
    }

    pub(crate) async fn list_tasks(
        &self,
        principal_id: PrincipalId,
    ) -> Result<Vec<TaskSummary>, ControlError> {
        let path = Arc::clone(&self.path);
        blocking(move || {
            let connection = open_connection(&path)?;
            let mut statement = connection
                .prepare(
                    "SELECT task_id, target_json FROM tasks
                     WHERE principal_id = ?1 ORDER BY task_id",
                )
                .map_err(sqlite_error)?;
            let rows = statement
                .query_map([principal_id.to_string()], |row| {
                    Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
                })
                .map_err(sqlite_error)?;
            let mut tasks = Vec::new();
            for row in rows {
                let (task_id, target_json) = row.map_err(sqlite_error)?;
                tasks.push(TaskSummary {
                    task_id: TaskId::from_uuid(task_id.parse().map_err(id_error)?),
                    target: serde_json::from_str(&target_json).map_err(json_error)?,
                });
            }
            Ok(tasks)
        })
        .await
    }

    pub(crate) async fn load_task(&self, task_id: TaskId) -> Result<StoredTask, ControlError> {
        let path = Arc::clone(&self.path);
        blocking(move || {
            let connection = open_connection(&path)?;
            load_task(&connection, task_id)
        })
        .await
    }

    pub(crate) async fn load_task_for_principal(
        &self,
        task_id: TaskId,
        principal_id: PrincipalId,
    ) -> Result<StoredTask, ControlError> {
        let task = self.load_task(task_id).await?;
        if task.principal_id != principal_id {
            return Err(task_not_found(task_id));
        }
        Ok(task)
    }

    pub(crate) async fn admit_command(
        &self,
        task_id: TaskId,
        principal_id: PrincipalId,
        surface: SurfaceRef,
        command_id: CommandId,
        input: CommandInput,
        allow_new: bool,
    ) -> Result<CommandAdmission, ControlError> {
        let path = Arc::clone(&self.path);
        blocking(move || {
            let mut connection = open_connection(&path)?;
            let transaction = connection
                .transaction_with_behavior(TransactionBehavior::Immediate)
                .map_err(sqlite_error)?;
            let task = load_task(&transaction, task_id)?;
            if task.principal_id != principal_id {
                return Err(task_not_found(task_id));
            }
            let command = CommandEnvelope {
                command_id,
                principal_id,
                surface,
                target: task.target.clone(),
                input,
            };
            let command_json = serde_json::to_string(&command).map_err(json_error)?;
            let existing = transaction
                .query_row(
                    "SELECT command_json, EXISTS(
                        SELECT 1 FROM pending_executions
                        WHERE pending_executions.command_id = commands.command_id
                     )
                     FROM commands WHERE command_id = ?1",
                    [command_id.to_string()],
                    |row| Ok((row.get::<_, String>(0)?, row.get::<_, bool>(1)?)),
                )
                .optional()
                .map_err(sqlite_error)?;
            // A durable admission outranks today's availability policy so an
            // uncertain retry observes its original result.
            if let Some((stored_command, pending)) = existing {
                if stored_command != command_json {
                    return Err(ControlError::conflict(format!(
                        "command id {command_id} was already used with different content"
                    )));
                }
                return Ok(CommandAdmission::Existing { command, pending });
            }
            if !allow_new {
                return Ok(CommandAdmission::NotAdmitted);
            }

            transaction
                .execute(
                    "INSERT INTO commands (command_id, task_id, command_json)
                     VALUES (?1, ?2, ?3)",
                    params![command_id.to_string(), task_id.to_string(), command_json,],
                )
                .map_err(sqlite_error)?;
            let event = append_event(
                &transaction,
                task_id,
                &format!("command:{command_id}"),
                &TaskEventKind::CommandSubmitted {
                    command: command.clone(),
                },
            )?;
            transaction
                .execute(
                    "INSERT INTO pending_executions (command_id, task_sequence)
                     VALUES (?1, ?2)",
                    params![command_id.to_string(), to_sql_integer(event.sequence)?],
                )
                .map_err(sqlite_error)?;
            transaction.commit().map_err(sqlite_error)?;
            Ok(CommandAdmission::Admitted {
                command,
                event: Box::new(event),
            })
        })
        .await
    }
}

fn load_task(connection: &Connection, task_id: TaskId) -> Result<StoredTask, ControlError> {
    let row = connection
        .query_row(
            "SELECT principal_id, node_id, target_json
             FROM tasks WHERE task_id = ?1",
            [task_id.to_string()],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                ))
            },
        )
        .optional()
        .map_err(sqlite_error)?
        .ok_or_else(|| task_not_found(task_id))?;
    Ok(StoredTask {
        principal_id: PrincipalId::from_uuid(row.0.parse().map_err(id_error)?),
        node_id: NodeId::from_uuid(row.1.parse().map_err(id_error)?),
        target: serde_json::from_str(&row.2).map_err(json_error)?,
    })
}

pub(crate) fn task_not_found(task_id: TaskId) -> ControlError {
    ControlError::not_found(format!("task {task_id} was not found"))
}

pub(crate) async fn blocking<T, F>(operation: F) -> Result<T, ControlError>
where
    T: Send + 'static,
    F: FnOnce() -> Result<T, ControlError> + Send + 'static,
{
    tokio::task::spawn_blocking(operation)
        .await
        .map_err(|error| ControlError::store(format!("SQLite worker failed: {error}")))?
}

pub(crate) fn to_sql_integer(value: u64) -> Result<i64, ControlError> {
    i64::try_from(value).map_err(|_| ControlError::store("integer exceeds SQLite i64 range"))
}

#[allow(
    clippy::needless_pass_by_value,
    reason = "the owned signature is required by Result::map_err"
)]
pub(crate) fn sqlite_error(error: rusqlite::Error) -> ControlError {
    ControlError::store(format!("SQLite error: {error}"))
}

#[allow(
    clippy::needless_pass_by_value,
    reason = "the owned signature is required by Result::map_err"
)]
pub(crate) fn json_error(error: serde_json::Error) -> ControlError {
    ControlError::store(format!("control serialization error: {error}"))
}

#[allow(
    clippy::needless_pass_by_value,
    reason = "the owned signature is required by Result::map_err"
)]
pub(crate) fn id_error(error: uuid::Error) -> ControlError {
    ControlError::store(format!("invalid stored identity: {error}"))
}
