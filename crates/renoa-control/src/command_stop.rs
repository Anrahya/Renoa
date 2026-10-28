//! A surface asks to stop one command's execution.
//!
//! The command is the stable identity: stopping it twice is one stop, and a
//! stop can never reach a later command of the task. The coordinator records
//! the stop before it answers and delivers it to the task's node until the
//! node acknowledges that it durably recorded it. Unlike a new command, a
//! stop is accepted while the node is offline: it can only cancel, and a Host
//! turn can outlive the node's link to the coordinator.

use std::sync::Arc;

use renoa_protocol::{CommandId, PrincipalId};
use rusqlite::{OptionalExtension as _, TransactionBehavior, params};

use crate::{
    ControlError, NodeId, ServerMessage, StopOutcome, TaskId, control_log,
    control_schema::open_connection,
    coordinator::CoordinatorState,
    store::{ControlStore, blocking, id_error, sqlite_error},
};

/// What recording a stop found.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum StopAdmission {
    /// The command's execution had already ended; nothing was recorded.
    AlreadyEnded,
    /// The stop is durable. `delivered` is whether the node already
    /// acknowledged it.
    Recorded { delivered: bool },
}

/// A recorded stop the node has not acknowledged.
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct PendingStop {
    pub(crate) task_id: TaskId,
    pub(crate) command_id: CommandId,
}

pub(crate) async fn stop_command(
    state: &CoordinatorState,
    principal_id: PrincipalId,
    task_id: TaskId,
    command_id: CommandId,
) -> Result<StopOutcome, ControlError> {
    let task = state
        .store
        .load_task_for_principal(task_id, principal_id)
        .await?;
    // The lifecycle lock orders this admission against a node activation, as
    // for a command: a stop recorded after an activation took its pending
    // snapshot must find that node's connection to deliver to.
    let lifecycle = state.connection_lifecycle.lock().await;
    let node = state.nodes.lock().await.get(&task.node_id).cloned();
    let admission = state.store.admit_stop(task_id, command_id).await?;
    drop(lifecycle);
    let delivered = match admission {
        StopAdmission::AlreadyEnded => return Ok(StopOutcome::AlreadyEnded),
        StopAdmission::Recorded { delivered } => delivered,
    };
    control_log::event(
        "info",
        "stop_recorded",
        &serde_json::json!({
            "task_id": task_id,
            "command_id": command_id,
            "node_id": task.node_id,
            "delivered": delivered,
            "node_online": node.is_some(),
        }),
    );
    if !delivered && let Some(node) = node {
        let _ = node
            .outgoing
            .send(ServerMessage::Stop {
                task_id,
                command_id,
            })
            .await;
    }
    Ok(StopOutcome::Requested)
}

impl ControlStore {
    /// Records a stop for `command_id` of `task_id`. An earlier record
    /// outranks the command's present state, so a retry observes its original
    /// answer; a command that already ended with no earlier record answers
    /// `AlreadyEnded` and gets no row.
    pub(crate) async fn admit_stop(
        &self,
        task_id: TaskId,
        command_id: CommandId,
    ) -> Result<StopAdmission, ControlError> {
        let path = Arc::clone(&self.path);
        blocking(move || {
            let mut connection = open_connection(&path)?;
            let transaction = connection
                .transaction_with_behavior(TransactionBehavior::Immediate)
                .map_err(sqlite_error)?;
            let settled = transaction
                .query_row(
                    "SELECT settled FROM command_states
                     WHERE command_id = ?1 AND task_id = ?2",
                    params![command_id.to_string(), task_id.to_string()],
                    |row| row.get::<_, bool>(0),
                )
                .optional()
                .map_err(sqlite_error)?
                .ok_or_else(|| {
                    ControlError::not_found(format!(
                        "command {command_id} was not admitted for task {task_id}"
                    ))
                })?;
            let recorded = transaction
                .query_row(
                    "SELECT delivered FROM stop_requests WHERE command_id = ?1",
                    [command_id.to_string()],
                    |row| row.get::<_, bool>(0),
                )
                .optional()
                .map_err(sqlite_error)?;
            if let Some(delivered) = recorded {
                return Ok(StopAdmission::Recorded { delivered });
            }
            if settled {
                return Ok(StopAdmission::AlreadyEnded);
            }
            transaction
                .execute(
                    "INSERT INTO stop_requests (command_id) VALUES (?1)",
                    [command_id.to_string()],
                )
                .map_err(sqlite_error)?;
            transaction.commit().map_err(sqlite_error)?;
            Ok(StopAdmission::Recorded { delivered: false })
        })
        .await
    }

    /// The stops `node_id` has not acknowledged, in the order they were
    /// recorded within each task.
    pub(crate) async fn load_pending_stops(
        &self,
        node_id: NodeId,
    ) -> Result<Vec<PendingStop>, ControlError> {
        let path = Arc::clone(&self.path);
        blocking(move || {
            let connection = open_connection(&path)?;
            let mut statement = connection
                .prepare(
                    "SELECT commands.task_id, stop_requests.command_id
                     FROM stop_requests
                     JOIN commands USING (command_id)
                     JOIN tasks USING (task_id)
                     WHERE tasks.node_id = ?1 AND stop_requests.delivered = 0
                     ORDER BY commands.task_id, stop_requests.rowid",
                )
                .map_err(sqlite_error)?;
            let rows = statement
                .query_map([node_id.to_string()], |row| {
                    Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
                })
                .map_err(sqlite_error)?;
            let mut pending = Vec::new();
            for row in rows {
                let (task_id, command_id) = row.map_err(sqlite_error)?;
                pending.push(PendingStop {
                    task_id: TaskId::from_uuid(task_id.parse().map_err(id_error)?),
                    command_id: CommandId::from_uuid(command_id.parse().map_err(id_error)?),
                });
            }
            Ok(pending)
        })
        .await
    }

    /// Marks a stop delivered once its node durably recorded it. The
    /// acknowledgement is idempotent, and one for a task that no longer exists
    /// has nothing left to confirm.
    pub(crate) async fn acknowledge_stop(
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
                    "SELECT node_id FROM tasks WHERE task_id = ?1",
                    [task_id.to_string()],
                    |row| row.get::<_, String>(0),
                )
                .optional()
                .map_err(sqlite_error)?;
            let Some(owner) = owner else {
                return Ok(());
            };
            if owner != node_id.to_string() {
                return Err(ControlError::invalid(format!(
                    "node {node_id} does not own task {task_id}"
                )));
            }
            transaction
                .execute(
                    "UPDATE stop_requests SET delivered = 1
                     WHERE command_id = ?1
                       AND command_id IN (SELECT command_id FROM commands WHERE task_id = ?2)",
                    params![command_id.to_string(), task_id.to_string()],
                )
                .map_err(sqlite_error)?;
            transaction.commit().map_err(sqlite_error)
        })
        .await
    }
}
