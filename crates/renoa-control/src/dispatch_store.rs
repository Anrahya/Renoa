use std::sync::Arc;

use renoa_protocol::{CommandEnvelope, CommandId};
use rusqlite::{OptionalExtension, TransactionBehavior, params};

use crate::{
    ControlError, NodeId, TaskId,
    control_schema::open_connection,
    store::{ControlStore, blocking, id_error, json_error, sqlite_error},
};

#[derive(Debug)]
pub(crate) struct PendingExecution {
    pub(crate) task_id: TaskId,
    pub(crate) command: CommandEnvelope,
}

impl ControlStore {
    pub(crate) async fn load_pending_executions(
        &self,
        node_id: NodeId,
    ) -> Result<Vec<PendingExecution>, ControlError> {
        let path = Arc::clone(&self.path);
        blocking(move || {
            let connection = open_connection(&path)?;
            let mut statement = connection
                .prepare(
                    "SELECT commands.task_id, commands.command_json
                     FROM pending_executions
                     JOIN commands USING (command_id)
                     JOIN tasks USING (task_id)
                     WHERE tasks.node_id = ?1
                     ORDER BY commands.task_id, pending_executions.task_sequence",
                )
                .map_err(sqlite_error)?;
            let rows = statement
                .query_map([node_id.to_string()], |row| {
                    Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
                })
                .map_err(sqlite_error)?;
            let mut pending = Vec::new();
            for row in rows {
                let (task_id, command_json) = row.map_err(sqlite_error)?;
                pending.push(PendingExecution {
                    task_id: TaskId::from_uuid(task_id.parse().map_err(id_error)?),
                    command: serde_json::from_str(&command_json).map_err(json_error)?,
                });
            }
            Ok(pending)
        })
        .await
    }

    pub(crate) async fn acknowledge_execution(
        &self,
        node_id: NodeId,
        task_id: TaskId,
        command_id: CommandId,
    ) -> Result<(), ControlError> {
        let path = Arc::clone(&self.path);
        blocking(move || {
            let mut connection = open_connection(&path)?;
            let transaction = connection
                .transaction_with_behavior(TransactionBehavior::Immediate)
                .map_err(sqlite_error)?;
            let owner = transaction
                .query_row(
                    "SELECT tasks.node_id
                     FROM commands
                     JOIN tasks USING (task_id)
                     WHERE commands.command_id = ?1 AND commands.task_id = ?2",
                    params![command_id.to_string(), task_id.to_string()],
                    |row| row.get::<_, String>(0),
                )
                .optional()
                .map_err(sqlite_error)?
                .ok_or_else(|| {
                    ControlError::not_found(format!(
                        "command {command_id} was not admitted for task {task_id}"
                    ))
                })?;
            let owner = NodeId::from_uuid(owner.parse().map_err(id_error)?);
            if owner != node_id {
                return Err(ControlError::invalid(format!(
                    "node {node_id} does not own task {task_id}"
                )));
            }
            transaction
                .execute(
                    "DELETE FROM pending_executions WHERE command_id = ?1",
                    [command_id.to_string()],
                )
                .map_err(sqlite_error)?;
            transaction.commit().map_err(sqlite_error)
        })
        .await
    }
}
