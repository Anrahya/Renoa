use renoa_protocol::{CommandId, ExecutionEvent};
use tokio::sync::mpsc;
use uuid::Uuid;

use crate::{
    ControlError, ErrorCode, NodeId, ServerMessage, TaskId,
    coordinator::CoordinatorState,
    operations::NodeOperation,
    wire::{publish_task_event, send_control_error, send_error},
};

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
        send_error(
            outgoing,
            None,
            ErrorCode::InvalidRole,
            "node connection has been replaced",
        )
        .await;
        return;
    }
    match operation {
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
