use renoa_protocol::{CommandId, ExecutionEvent};
use tokio::sync::mpsc;
use uuid::Uuid;

use crate::{
    ControlError, ErrorCode, NodeId, ServerMessage, TaskId, control_log,
    coordinator::CoordinatorState,
    host_calls::HostReply,
    operations::NodeOperation,
    wire::{publish_task_event, send_control_error, send_error},
};

const REPLACED: &str = "node connection has been replaced";

/// Marks the node's current connection as answering Host control calls. The
/// connection is checked and marked under one lock, so a connection that a
/// newer one replaced cannot mark the newer one's entry. Returns whether the
/// connection was the current one.
pub(crate) async fn serve_host_control(
    state: &CoordinatorState,
    node_id: NodeId,
    connection_id: Uuid,
) -> bool {
    state
        .nodes
        .lock()
        .await
        .get_mut(&node_id)
        .filter(|node| node.connection_id == connection_id)
        .map(|node| node.host_control = true)
        .is_some()
}

pub(crate) async fn handle_node_operation(
    state: &CoordinatorState,
    outgoing: &mpsc::Sender<ServerMessage>,
    node_id: NodeId,
    connection_id: Uuid,
    operation: NodeOperation,
) {
    let current = state
        .nodes
        .lock()
        .await
        .get(&node_id)
        .is_some_and(|node| node.connection_id == connection_id);
    if !current {
        send_error(outgoing, None, ErrorCode::InvalidRole, REPLACED).await;
        return;
    }
    match operation {
        NodeOperation::ServeHostControl => {
            if serve_host_control(state, node_id, connection_id).await {
                control_log::event(
                    "info",
                    "node_host_control_advertised",
                    &serde_json::json!({ "node_id": node_id }),
                );
            } else {
                send_error(outgoing, None, ErrorCode::InvalidRole, REPLACED).await;
            }
        }
        NodeOperation::CompleteHostCall {
            call_id,
            status,
            body,
        } => {
            let delivered =
                state
                    .host_calls
                    .complete(connection_id, call_id, HostReply { status, body });
            if !delivered {
                control_log::event(
                    "info",
                    "host_call_answer_dropped",
                    &serde_json::json!({ "node_id": node_id, "call_id": call_id }),
                );
            }
        }
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
        NodeOperation::AcknowledgeStop {
            task_id,
            command_id,
        } => {
            if let Err(error) = state
                .store
                .acknowledge_stop(node_id, task_id, command_id)
                .await
            {
                send_control_error(outgoing, None, &error).await;
            }
        }
        NodeOperation::PublishLiveActivity {
            task_id,
            command_id,
            activity,
        } => crate::live_activity::relay(state, node_id, task_id, command_id, activity).await,
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
    let admission = state
        .store
        .append_execution_events(task_id, command_id, events)
        .await?;
    for event in admission.events {
        publish_task_event(state, event).await;
    }
    Ok(admission.through_execution_sequence)
}
