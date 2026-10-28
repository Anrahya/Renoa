//! The answer to an agent's request, returned to the asker's task.
//!
//! When the execution of a command whose origin is a request ends, the
//! coordinator admits a new command in the task that holds the asker's
//! command. It does so in the transaction that records the ending, under an
//! identity derived from the request, so a retried batch cannot answer twice
//! and a lost connection cannot lose the answer. An answer starts nothing by
//! itself: only an explicit message continues a chain.

use renoa_protocol::{
    AgentMessageKind, AgentOrigin, CommandEnvelope, CommandId, CommandInput, ExecutionEventKind,
    ExecutionTerminal, SurfaceRef, TargetRef,
};
use rusqlite::{OptionalExtension as _, Transaction};

use crate::{
    ControlError, NodeId, ServerMessage, TaskEvent, TaskEventKind, TaskId,
    agent_messages::{AGENT_SURFACE, MAX_MESSAGE_BYTES, derived_id},
    control_log,
    coordinator::{CoordinatorState, NodeConnection},
    store::{id_error, insert_command, json_error, load_task, sqlite_error},
    wire::publish_task_event,
};

/// An answer admitted in the asker's task.
#[derive(Debug)]
pub(crate) struct AgentReply {
    pub(crate) task_id: TaskId,
    /// The node that executes the asker's task.
    pub(crate) node_id: NodeId,
    pub(crate) command: CommandEnvelope,
    pub(crate) event: TaskEvent,
}

/// The name `node_id` advertises for `target`, or the target itself when the
/// agent is no longer advertised.
pub(crate) async fn name_of(
    state: &CoordinatorState,
    node_id: NodeId,
    target: &TargetRef,
) -> String {
    state
        .nodes
        .lock()
        .await
        .get(&node_id)
        .and_then(|node| node.targets.iter().find(|card| &card.target == target))
        .map_or_else(|| target.as_str().to_owned(), |card| card.name.clone())
}

/// Publishes a committed answer and delivers it to the asker's node when that
/// node is online. Otherwise the pending delivery waits for its reconnect.
pub(crate) async fn dispatch(
    state: &CoordinatorState,
    reply: AgentReply,
    node: Option<NodeConnection>,
) {
    control_log::event(
        "info",
        "agent_reply_admitted",
        &serde_json::json!({
            "reply_id": reply.command.command_id,
            "task_id": reply.task_id,
            "node_id": reply.node_id,
            "origin": reply.command.origin,
            "dispatched": node.is_some(),
        }),
    );
    publish_task_event(state, reply.event).await;
    if let Some(node) = node {
        let _ = node
            .outgoing
            .send(ServerMessage::Execute {
                task_id: reply.task_id,
                command: reply.command,
            })
            .await;
    }
}

/// Admits the answer to `command_id` when `appended` holds its terminal event
/// and the command was an agent's request. Nothing is written otherwise.
pub(crate) fn admit_reply(
    transaction: &Transaction<'_>,
    task_id: TaskId,
    command_id: CommandId,
    appended: &[TaskEvent],
    replier: &str,
) -> Result<Option<AgentReply>, ControlError> {
    let Some(terminal) = appended.iter().find_map(|event| match &event.kind {
        TaskEventKind::ExecutionEvent { event, .. } => match &event.kind {
            ExecutionEventKind::ExecutionTerminated { terminal } => Some(terminal.clone()),
            _ => None,
        },
        TaskEventKind::CommandSubmitted { .. } => None,
    }) else {
        return Ok(None);
    };
    let command_json = transaction
        .query_row(
            "SELECT command_json FROM commands WHERE command_id = ?1",
            [command_id.to_string()],
            |row| row.get::<_, String>(0),
        )
        .map_err(sqlite_error)?;
    let command: CommandEnvelope = serde_json::from_str(&command_json).map_err(json_error)?;
    let Some(origin) = command.origin.filter(|origin| origin.kind.expects_reply()) else {
        return Ok(None);
    };
    let asker_task = transaction
        .query_row(
            "SELECT task_id FROM commands WHERE command_id = ?1",
            [origin.from_command.to_string()],
            |row| row.get::<_, String>(0),
        )
        .optional()
        .map_err(sqlite_error)?;
    let Some(asker_task) = asker_task else {
        control_log::event(
            "warn",
            "agent_reply_dropped",
            &serde_json::json!({
                "command_id": command_id,
                "from_command": origin.from_command,
                "reason": "the asking command no longer exists",
            }),
        );
        return Ok(None);
    };
    let asker_task = TaskId::from_uuid(asker_task.parse().map_err(id_error)?);
    // Only the batch that first records the terminal event reaches here, so
    // the derived identity is used once per request.
    let reply_id = CommandId::from_uuid(derived_id(&format!("renoa.agent.reply.v1:{command_id}")));
    let (kind, text) = match terminal {
        ExecutionTerminal::Completed => (
            AgentMessageKind::Reply,
            last_assistant_text(transaction, task_id, command_id)?.unwrap_or_default(),
        ),
        ExecutionTerminal::Failed { error } => (AgentMessageKind::Failure, error),
        ExecutionTerminal::Cancelled { reason } => (AgentMessageKind::Failure, reason),
    };
    // An answer is bounded like a request, so one oversized message can never
    // outgrow the frame that delivers it.
    let mut text = text;
    let mut end = text.len().min(MAX_MESSAGE_BYTES);
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    text.truncate(end);
    let asker = load_task(transaction, asker_task)?;
    let replier_target = load_task(transaction, task_id)?.target;
    let reply = CommandEnvelope {
        command_id: reply_id,
        principal_id: asker.principal_id,
        surface: SurfaceRef::new(AGENT_SURFACE),
        target: asker.target,
        input: CommandInput::from_text(text),
        origin: Some(AgentOrigin {
            from_command: command_id,
            from: replier_target,
            name: replier.to_owned(),
            depth: origin.depth.saturating_add(1),
            kind,
        }),
    };
    let reply_json = serde_json::to_string(&reply).map_err(json_error)?;
    let event = insert_command(transaction, asker_task, &reply, &reply_json)?;
    Ok(Some(AgentReply {
        task_id: asker_task,
        node_id: asker.node_id,
        command: reply,
        event,
    }))
}

/// The text of the last assistant message the execution of `command_id`
/// recorded in `task_id`.
fn last_assistant_text(
    transaction: &Transaction<'_>,
    task_id: TaskId,
    command_id: CommandId,
) -> Result<Option<String>, ControlError> {
    transaction
        .query_row(
            "SELECT json_extract(kind_json, '$.event.kind.text') FROM task_events
             WHERE task_id = ?1
               AND json_extract(kind_json, '$.commandId') = ?2
               AND json_extract(kind_json, '$.event.kind.type') = 'assistant_message'
             ORDER BY sequence DESC LIMIT 1",
            [task_id.to_string(), command_id.to_string()],
            |row| row.get::<_, String>(0),
        )
        .optional()
        .map_err(sqlite_error)
}
