//! What the node's own automation surface reads from the ledger: which task a
//! Host session belongs to, and how a command it submitted ended.

use std::sync::Arc;

use renoa_control::TaskId;
use renoa_protocol::{CommandId, ExecutionEvent, ExecutionEventKind, ExecutionTerminal};
use rusqlite::OptionalExtension as _;
use uuid::Uuid;

use super::{NodeStore, NodeStoreError, blocking, parse_uuid, schema::open_connection};

/// How a finished command ended, with the last assistant text it recorded
/// and how many of its tool calls returned an error.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct CommandOutcome {
    pub(crate) terminal: ExecutionTerminal,
    pub(crate) answer: Option<String>,
    pub(crate) failed_tool_calls: u32,
}

impl NodeStore {
    /// The task whose commands execute in `session`. A Host session belongs to
    /// at most one task.
    pub(crate) async fn task_for_session(
        &self,
        session: Uuid,
    ) -> Result<Option<TaskId>, NodeStoreError> {
        let path = Arc::clone(&self.path);
        blocking(move || {
            let task = open_connection(&path)?
                .query_row(
                    "SELECT task_id FROM host_node_tasks WHERE session_id = ?1",
                    [session.to_string()],
                    |row| row.get::<_, String>(0),
                )
                .optional()?;
            task.map(|task| Ok(TaskId::from_uuid(parse_uuid(&task, "task")?)))
                .transpose()
        })
        .await
    }

    /// How `command_id` ended, once the ledger holds its terminal event.
    pub(crate) async fn outcome(
        &self,
        command_id: CommandId,
    ) -> Result<Option<CommandOutcome>, NodeStoreError> {
        let path = Arc::clone(&self.path);
        blocking(move || {
            let connection = open_connection(&path)?;
            let terminal = connection
                .query_row(
                    "SELECT terminal FROM host_node_executions WHERE command_id = ?1",
                    [command_id.to_string()],
                    |row| row.get::<_, bool>(0),
                )
                .optional()?;
            if terminal != Some(true) {
                return Ok(None);
            }
            let mut statement = connection.prepare(
                "SELECT event_json FROM host_node_events WHERE command_id = ?1 ORDER BY sequence",
            )?;
            let rows =
                statement.query_map([command_id.to_string()], |row| row.get::<_, String>(0))?;
            let mut answer = None;
            let mut ended = None;
            let mut failed_tool_calls = 0_u32;
            for row in rows {
                let event: ExecutionEvent = serde_json::from_str(&row?)?;
                match event.kind {
                    ExecutionEventKind::AssistantMessage { text } => answer = Some(text),
                    ExecutionEventKind::ExecutionTerminated { terminal } => ended = Some(terminal),
                    ExecutionEventKind::ToolFinished { is_error: true, .. } => {
                        failed_tool_calls = failed_tool_calls.saturating_add(1);
                    }
                    _ => {}
                }
            }
            let terminal = ended.ok_or_else(|| {
                NodeStoreError::Invalid(format!(
                    "terminal execution for command {command_id} has no terminal event"
                ))
            })?;
            Ok(Some(CommandOutcome {
                terminal,
                answer,
                failed_tool_calls,
            }))
        })
        .await
    }
}
