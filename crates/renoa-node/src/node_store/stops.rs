//! Stops the owner asked for, recorded before the node acknowledges them.
//!
//! A stop is keyed by its command and may precede the command's admission:
//! the coordinator can deliver a stop before the `execute` it follows when
//! another surface races the command's first delivery, and the node must still
//! start that command cancelled.

use std::sync::Arc;

use renoa_control::TaskId;
use renoa_protocol::CommandId;
use rusqlite::TransactionBehavior;

use super::{NodeStore, NodeStoreError, blocking, records::load_record, schema::open_connection};

/// What recording a stop found.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum StopRecord {
    /// The stop is durable. The command may be running, queued, or not yet
    /// admitted, and it starts cancelled if it has not started.
    Recorded,
    /// The command's execution had already terminated. Nothing was recorded.
    AlreadyEnded,
}

impl NodeStore {
    /// Records a stop for `command_id` of `task_id`. Repeating it converges.
    pub(crate) async fn record_stop(
        &self,
        task_id: TaskId,
        command_id: CommandId,
    ) -> Result<StopRecord, NodeStoreError> {
        let path = Arc::clone(&self.path);
        blocking(move || {
            let mut connection = open_connection(&path)?;
            let transaction =
                connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
            if let Some(record) = load_record(&transaction, command_id)? {
                if record.task_id != task_id {
                    return Err(NodeStoreError::Invalid(format!(
                        "command {command_id} does not belong to task {task_id}"
                    )));
                }
                if record.terminal {
                    return Ok(StopRecord::AlreadyEnded);
                }
            }
            transaction.execute(
                "INSERT OR IGNORE INTO host_node_stops (command_id, task_id) VALUES (?1, ?2)",
                [command_id.to_string(), task_id.to_string()],
            )?;
            transaction.commit()?;
            Ok(StopRecord::Recorded)
        })
        .await
    }

    /// Whether a stop is recorded for `command_id`.
    pub(crate) async fn stop_requested(
        &self,
        command_id: CommandId,
    ) -> Result<bool, NodeStoreError> {
        let path = Arc::clone(&self.path);
        blocking(move || {
            Ok(open_connection(&path)?.query_row(
                "SELECT EXISTS(SELECT 1 FROM host_node_stops WHERE command_id = ?1)",
                [command_id.to_string()],
                |row| row.get(0),
            )?)
        })
        .await
    }
}
