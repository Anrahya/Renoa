use renoa_protocol::{CommandId, ExecutionEvent, ExecutionEventKind};
use tokio::sync::mpsc;
use uuid::Uuid;

use crate::{
    ControlError, ErrorCode, NodeId, ServerMessage, TaskId,
    coordinator::CoordinatorState,
    operations::NodeOperation,
    wire::{publish_task_event, send_control_error, send_error},
};

/// Whether `connection_id` is still the connection serving `node_id`; a node
/// that reconnected replaces its earlier connection.
pub(crate) async fn node_connection_is_current(
    state: &CoordinatorState,
    node_id: NodeId,
    connection_id: Uuid,
) -> bool {
    state
        .nodes
        .lock()
        .await
        .get(&node_id)
        .is_some_and(|node| node.connection_id == connection_id)
}

pub(crate) async fn handle_node_operation(
    state: &CoordinatorState,
    outgoing: &mpsc::Sender<ServerMessage>,
    node_id: NodeId,
    connection_id: Uuid,
    operation: NodeOperation,
) {
    if !node_connection_is_current(state, node_id, connection_id).await {
        // A request names its own id so the sender's wait ends with the reason.
        let request_id = match &operation {
            NodeOperation::SendAgentMessage { request_id, .. } => Some(*request_id),
            _ => None,
        };
        send_error(
            outgoing,
            request_id,
            ErrorCode::InvalidRole,
            "node connection has been replaced",
        )
        .await;
        return;
    }
    match operation {
        NodeOperation::AdvertiseTargets { targets } => {
            if let Err(error) =
                crate::task_opening::advertise_targets(state, node_id, connection_id, targets).await
            {
                send_control_error(outgoing, None, &error).await;
            }
        }
        NodeOperation::AcknowledgeExecution {
            task_id,
            command_id,
        } => {
            let result = state
                .store
                .acknowledge_execution(node_id, task_id, command_id)
                .await;
            match result {
                Ok(()) => {
                    let _ = outgoing
                        .send(ServerMessage::ExecutionAcknowledged { command_id })
                        .await;
                }
                Err(error) => send_control_error(outgoing, None, &error).await,
            }
        }
        NodeOperation::PublishExecutionEvents {
            task_id,
            command_id,
            events,
        } => {
            let result =
                publish_execution_events(state, node_id, task_id, command_id, events).await;
            match result {
                Ok(through_execution_sequence) => {
                    let _ = outgoing
                        .send(ServerMessage::ExecutionEventsAccepted {
                            command_id,
                            through_execution_sequence,
                        })
                        .await;
                }
                Err(error) => send_control_error(outgoing, None, &error).await,
            }
        }
        NodeOperation::SendAgentMessage {
            request_id,
            message,
        } => {
            crate::agent_messages::send_agent_message(
                state, outgoing, node_id, request_id, message,
            )
            .await;
        }
    }
}

async fn publish_execution_events(
    state: &CoordinatorState,
    node_id: NodeId,
    task_id: TaskId,
    command_id: CommandId,
    events: Vec<ExecutionEvent>,
) -> Result<u64, ControlError> {
    let task = state.store.load_task(task_id).await?;
    if task.node_id != node_id {
        return Err(ControlError::invalid(format!(
            "node {node_id} does not own task {task_id}"
        )));
    }
    // A terminal event may admit the answer to an agent's request in another
    // task. As in `submit_command`, that admission and the lookup of the node
    // that must receive it happen under the lifecycle lock, so a node
    // activating between the two still gets the pending delivery.
    let ends = events
        .last()
        .is_some_and(|event| matches!(event.kind, ExecutionEventKind::ExecutionTerminated { .. }));
    let lifecycle = if ends {
        Some(state.connection_lifecycle.lock().await)
    } else {
        None
    };
    let replier = if ends {
        crate::agent_replies::name_of(state, node_id, &task.target).await
    } else {
        String::new()
    };
    let admission = state
        .store
        .append_execution_events(task_id, command_id, events, replier)
        .await?;
    let reply_node = match &admission.reply {
        Some(reply) => state.nodes.lock().await.get(&reply.node_id).cloned(),
        None => None,
    };
    drop(lifecycle);
    for event in admission.events {
        publish_task_event(state, event).await;
    }
    if let Some(reply) = admission.reply {
        crate::agent_replies::dispatch(state, reply, reply_node).await;
    }
    Ok(admission.through_execution_sequence)
}
