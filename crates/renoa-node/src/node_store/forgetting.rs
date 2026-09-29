//! Forgetting a task the node executed: an automation's own conversation is
//! deleted with its automation, and the node keeps none of it.

use std::sync::Arc;

use renoa_control::TaskId;
use rusqlite::{OptionalExtension as _, TransactionBehavior};
use uuid::Uuid;

use super::{NodeStore, NodeStoreError, blocking, parse_uuid, schema::open_connection};

impl NodeStore {
    /// The agent and Host session `task_id` executes in, if this node ran it.
    /// A task with an execution that has not terminated is refused, so nothing
    /// of it is deleted while it may still run.
    pub(crate) async fn task_session(
        &self,
        task_id: TaskId,
    ) -> Result<Option<(Uuid, Uuid)>, NodeStoreError> {
        let path = Arc::clone(&self.path);
        blocking(move || {
            let connection = open_connection(&path)?;
            let task = task_id.to_string();
            let running: bool = connection.query_row(
                "SELECT EXISTS(SELECT 1 FROM host_node_executions WHERE task_id = ?1 AND terminal = 0)",
                [&task],
                |row| row.get(0),
            )?;
            if running {
                return Err(NodeStoreError::Invalid(format!(
                    "task {task_id} has an execution that has not terminated"
                )));
            }
            let binding = connection
                .query_row(
                    "SELECT agent_id, session_id FROM host_node_tasks WHERE task_id = ?1",
                    [&task],
                    |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
                )
                .optional()?;
            binding
                .map(|(agent, session)| {
                    Ok((
                        parse_uuid(&agent, "agent")?,
                        parse_uuid(&session, "session")?,
                    ))
                })
                .transpose()
        })
        .await
    }

    /// Deletes `task_id` with its executions and their events. It does not
    /// check for an unfinished execution; [`Self::task_session`] does, before
    /// anything of the task is deleted.
    pub(crate) async fn forget_task(&self, task_id: TaskId) -> Result<(), NodeStoreError> {
        let path = Arc::clone(&self.path);
        blocking(move || {
            let mut connection = open_connection(&path)?;
            let transaction =
                connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
            let task = task_id.to_string();
            transaction.execute(
                "DELETE FROM host_node_events WHERE command_id IN
                    (SELECT command_id FROM host_node_executions WHERE task_id = ?1)",
                [&task],
            )?;
            transaction.execute(
                "DELETE FROM host_node_executions WHERE task_id = ?1",
                [&task],
            )?;
            transaction.execute("DELETE FROM host_node_tasks WHERE task_id = ?1", [&task])?;
            transaction.commit()?;
            Ok(())
        })
        .await
    }
}
