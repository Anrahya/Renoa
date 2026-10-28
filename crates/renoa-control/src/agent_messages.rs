//! A node sends a message for an agent it executes to another agent of the
//! same owner.
//!
//! The message becomes a command in a thread task of the destination agent.
//! The coordinator derives who sent it, never the request: the sender is the
//! agent of the task that holds `from_command`, and that task must be bound to
//! the requesting node. The destination task is derived from the pair of
//! agents and opened in the same transaction as the command, so the first and
//! every later message reach one conversation, and a refusal leaves no task,
//! command or record. The answer comes back through [`crate::agent_replies`].

use std::sync::Arc;

use renoa_protocol::{
    AgentMessageKind, AgentOrigin, CommandEnvelope, CommandId, CommandInput, PrincipalId,
    SurfaceRef, TargetCard, TargetRef,
};
use rusqlite::{OptionalExtension as _, Transaction, TransactionBehavior};
use sha2::{Digest as _, Sha256};
use tokio::sync::mpsc;
use uuid::Uuid;

use crate::{
    ControlError, NodeId, ServerMessage, TaskEvent, TaskId, TaskSpec, control_log,
    control_schema::open_connection,
    coordinator::CoordinatorState,
    node_ownership::node_owner,
    store::{
        ControlStore, blocking, id_error, insert_command, insert_task, json_error, load_task,
        sqlite_error,
    },
    wire::{publish_task_event, send_control_error},
};

/// The deepest chain of agent requests. A request is refused beyond it. This
/// is a backstop against runaway exchanges, not a conversation limit, and no
/// agent is shown it.
pub(crate) const MAX_CHAIN_DEPTH: u32 = 100;
pub(crate) const MAX_MESSAGE_BYTES: usize = 64 * 1024;

/// What the surface field of a command from an agent says.
pub(crate) const AGENT_SURFACE: &str = "agent";

/// A message as the sending node asks for it.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct OutgoingMessage {
    /// The command the sending agent is executing.
    pub(crate) from_command: CommandId,
    /// The new command's stable identity.
    pub(crate) message_id: CommandId,
    pub(crate) to_node: NodeId,
    pub(crate) to_target: TargetRef,
    pub(crate) text: String,
}

#[derive(Debug)]
pub(crate) enum MessageAdmission {
    Admitted {
        task_id: TaskId,
        command: CommandEnvelope,
        event: Box<TaskEvent>,
    },
    Existing {
        task_id: TaskId,
        command: CommandEnvelope,
        pending: bool,
    },
}

/// Derives a stable identity from a domain-separated value.
pub(crate) fn derived_id(value: &str) -> Uuid {
    let digest = Sha256::digest(value);
    let mut bytes = [0; 16];
    bytes.copy_from_slice(&digest[..16]);
    Uuid::from_bytes(bytes)
}

/// The thread task in which `to` receives messages from `from`.
fn thread_task_id(
    principal_id: PrincipalId,
    from: (NodeId, &TargetRef),
    to: (NodeId, &TargetRef),
) -> TaskId {
    TaskId::from_uuid(derived_id(&format!(
        "renoa.agent.thread.v1:{principal_id}:{}:{}:{}:{}",
        from.0,
        from.1.as_str(),
        to.0,
        to.1.as_str()
    )))
}

/// Admits `message` for `node_id` and answers with `command_accepted`, or
/// with the reason it was refused.
pub(crate) async fn send_agent_message(
    state: &CoordinatorState,
    outgoing: &mpsc::Sender<ServerMessage>,
    node_id: NodeId,
    request_id: u64,
    message: OutgoingMessage,
) {
    let message_id = message.message_id;
    let lifecycle = state.connection_lifecycle.lock().await;
    let (sender_cards, destination) = {
        let nodes = state.nodes.lock().await;
        (
            nodes
                .get(&node_id)
                .map(|node| node.targets.clone())
                .unwrap_or_default(),
            nodes
                .get(&message.to_node)
                .map(|node| (node.targets.clone(), node.clone())),
        )
    };
    let admission = state
        .store
        .admit_agent_message(
            node_id,
            sender_cards,
            destination.as_ref().map(|(cards, _)| cards.clone()),
            message.clone(),
        )
        .await;
    drop(lifecycle);
    let admission = match admission {
        Ok(admission) => admission,
        Err(error) => {
            control_log::event(
                "warn",
                "agent_message_rejected",
                &serde_json::json!({
                    "message_id": message_id,
                    "from_command": message.from_command,
                    "node_id": node_id,
                    "to_node": message.to_node,
                    "to_target": message.to_target.as_str(),
                    "error": error.to_string(),
                }),
            );
            send_control_error(outgoing, Some(request_id), &error).await;
            return;
        }
    };
    let (task_id, command, event, pending) = match admission {
        MessageAdmission::Admitted {
            task_id,
            command,
            event,
        } => (task_id, command, Some(*event), true),
        MessageAdmission::Existing {
            task_id,
            command,
            pending,
        } => (task_id, command, None, pending),
    };
    control_log::event(
        "info",
        if event.is_some() {
            "agent_message_admitted"
        } else {
            "agent_message_replayed"
        },
        &serde_json::json!({
            "message_id": message_id,
            "from_command": message.from_command,
            "node_id": node_id,
            "to_node": message.to_node,
            "to_target": message.to_target.as_str(),
            "task_id": task_id,
            "depth": command.origin.as_ref().map(|origin| origin.depth),
            "dispatched": pending && destination.is_some(),
        }),
    );
    let _ = outgoing
        .send(ServerMessage::CommandAccepted {
            request_id,
            command_id: message_id,
        })
        .await;
    if let Some(event) = event {
        publish_task_event(state, event).await;
    }
    if let (true, Some((_, node))) = (pending, destination) {
        let _ = node
            .outgoing
            .send(ServerMessage::Execute { task_id, command })
            .await;
    }
}

impl ControlStore {
    /// `sender_cards` are the targets the requesting node advertises and
    /// `destination` those of the destination node, or `None` while it is
    /// offline.
    pub(crate) async fn admit_agent_message(
        &self,
        node_id: NodeId,
        sender_cards: Vec<TargetCard>,
        destination: Option<Vec<TargetCard>>,
        message: OutgoingMessage,
    ) -> Result<MessageAdmission, ControlError> {
        let path = Arc::clone(&self.path);
        blocking(move || {
            let mut connection = open_connection(&path)?;
            let transaction = connection
                .transaction_with_behavior(TransactionBehavior::Immediate)
                .map_err(sqlite_error)?;
            let admission = admit_in(
                &transaction,
                node_id,
                &sender_cards,
                destination.as_deref(),
                &message,
            )?;
            transaction.commit().map_err(sqlite_error)?;
            Ok(admission)
        })
        .await
    }
}

/// Runs every check before its first write; the caller commits only on `Ok`.
fn admit_in(
    transaction: &Transaction<'_>,
    node_id: NodeId,
    sender_cards: &[TargetCard],
    destination: Option<&[TargetCard]>,
    message: &OutgoingMessage,
) -> Result<MessageAdmission, ControlError> {
    let principal_id = node_owner(transaction, node_id)?.ok_or_else(|| {
        ControlError::role("this node has no owner, so its agents cannot message other agents")
    })?;
    let (source, source_command) = load_source(transaction, node_id, principal_id, message)?;
    let depth = message_depth(&source_command, message)?;
    if node_owner(transaction, message.to_node)? != Some(principal_id) {
        return Err(ControlError::not_found(
            "that agent is not one of your owner's agents",
        ));
    }
    if (node_id, &source.target) == (message.to_node, &message.to_target) {
        return Err(ControlError::invalid("an agent cannot message itself"));
    }
    let task_id = thread_task_id(
        principal_id,
        (node_id, &source.target),
        (message.to_node, &message.to_target),
    );
    let command_with = |name: String| CommandEnvelope {
        command_id: message.message_id,
        principal_id,
        surface: SurfaceRef::new(AGENT_SURFACE),
        target: message.to_target.clone(),
        input: CommandInput::from_text(message.text.clone()),
        origin: Some(AgentOrigin {
            from_command: message.from_command,
            from: source.target.clone(),
            name,
            depth,
            kind: AgentMessageKind::Request,
        }),
    };
    // A durable admission outranks today's availability, and a rename since,
    // so an uncertain retry observes its original result.
    if let Some((stored, stored_task, pending)) = stored_message(transaction, message.message_id)? {
        let stored_name = stored
            .origin
            .as_ref()
            .map(|origin| origin.name.clone())
            .unwrap_or_default();
        if stored != command_with(stored_name) || stored_task != task_id.to_string() {
            return Err(ControlError::conflict(format!(
                "message id {} was already used with different content",
                message.message_id
            )));
        }
        return Ok(MessageAdmission::Existing {
            task_id,
            command: stored,
            pending,
        });
    }
    let name = sender_cards
        .iter()
        .find(|card| card.target == source.target)
        .map(|card| card.name.clone())
        .ok_or_else(|| {
            ControlError::invalid("the sending agent is not advertised by its node, so the message cannot say who it is from")
        })?;
    let command = command_with(name);
    let command_json = serde_json::to_string(&command).map_err(json_error)?;
    match destination {
        None => {
            return Err(ControlError::node_offline_with(
                "that agent's machine is offline right now",
            ));
        }
        Some(cards) if !cards.iter().any(|card| card.target == message.to_target) => {
            return Err(ControlError::not_found(
                "that agent is no longer available on its machine",
            ));
        }
        Some(_) => {}
    }
    open_thread(
        transaction,
        &TaskSpec {
            task_id,
            principal_id,
            node_id: message.to_node,
            target: message.to_target.clone(),
        },
    )?;
    let event = insert_command(transaction, task_id, &command, &command_json)?;
    Ok(MessageAdmission::Admitted {
        task_id,
        command,
        event: Box::new(event),
    })
}

/// The text limits, and the depth the message would have in its chain: one
/// deeper than the command it is sent from.
fn message_depth(
    source_command: &CommandEnvelope,
    message: &OutgoingMessage,
) -> Result<u32, ControlError> {
    if message.text.trim().is_empty() {
        return Err(ControlError::invalid("the message text is empty"));
    }
    if message.text.len() > MAX_MESSAGE_BYTES {
        return Err(ControlError::invalid(format!(
            "the message text is {} bytes and the limit is {MAX_MESSAGE_BYTES}; shorten it",
            message.text.len()
        )));
    }
    let depth = source_command
        .origin
        .as_ref()
        .map_or(0, |origin| origin.depth)
        .saturating_add(1);
    if depth > MAX_CHAIN_DEPTH {
        return Err(ControlError::invalid(
            "this exchange between agents has gone on too long and was stopped; do not message \
             another agent for it, and tell the owner what you were trying to do",
        ));
    }
    Ok(depth)
}

/// The command `message_id` admitted earlier, the thread task it went to, and
/// whether its delivery is still pending.
fn stored_message(
    transaction: &Transaction<'_>,
    message_id: CommandId,
) -> Result<Option<(CommandEnvelope, String, bool)>, ControlError> {
    let row = transaction
        .query_row(
            "SELECT command_json, task_id, EXISTS(
                SELECT 1 FROM pending_executions
                WHERE pending_executions.command_id = commands.command_id
             ) FROM commands WHERE command_id = ?1",
            [message_id.to_string()],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, bool>(2)?,
                ))
            },
        )
        .optional()
        .map_err(sqlite_error)?;
    row.map(|(json, task, pending)| {
        serde_json::from_str(&json)
            .map(|command| (command, task, pending))
            .map_err(json_error)
    })
    .transpose()
}

/// The task and command `message.from_command` names, which must belong to
/// `principal_id` and be bound to `node_id`: a node speaks only for the
/// agents it executes.
fn load_source(
    transaction: &Transaction<'_>,
    node_id: NodeId,
    principal_id: PrincipalId,
    message: &OutgoingMessage,
) -> Result<(crate::store::StoredTask, CommandEnvelope), ControlError> {
    let not_found = || {
        ControlError::not_found(format!(
            "command {} is not one this node is executing",
            message.from_command
        ))
    };
    let row = transaction
        .query_row(
            "SELECT task_id, command_json FROM commands WHERE command_id = ?1",
            [message.from_command.to_string()],
            |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
        )
        .optional()
        .map_err(sqlite_error)?
        .ok_or_else(not_found)?;
    let task = load_task(
        transaction,
        TaskId::from_uuid(row.0.parse().map_err(id_error)?),
    )?;
    if task.node_id != node_id || task.principal_id != principal_id {
        return Err(not_found());
    }
    let command = serde_json::from_str(&row.1).map_err(json_error)?;
    Ok((task, command))
}

/// Opens the thread task, or checks that an earlier opening has the binding
/// this message needs.
fn open_thread(transaction: &Transaction<'_>, task: &TaskSpec) -> Result<(), ControlError> {
    let existing = transaction
        .query_row(
            "SELECT principal_id, node_id, target_json FROM tasks WHERE task_id = ?1",
            [task.task_id.to_string()],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                ))
            },
        )
        .optional()
        .map_err(sqlite_error)?;
    match existing {
        None => insert_task(transaction, task),
        Some(bound)
            if bound
                == (
                    task.principal_id.to_string(),
                    task.node_id.to_string(),
                    serde_json::to_string(&task.target).map_err(json_error)?,
                ) =>
        {
            Ok(())
        }
        Some(_) => Err(ControlError::conflict(format!(
            "task id {} is already used by a different task",
            task.task_id
        ))),
    }
}
