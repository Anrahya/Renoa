//! Operations an authenticated surface performs on its principal's tasks.

use std::sync::Arc;

use renoa_protocol::{CommandId, CommandInput, PrincipalId, SurfaceRef};
use tokio::sync::{broadcast, mpsc};
use tokio_util::sync::CancellationToken;

use crate::{
    ControlError, ErrorCode, ServerMessage, TaskId, TaskSpec, control_log,
    coordinator::CoordinatorState,
    device_admin::{self, SurfaceSession},
    operations::SurfaceOperation,
    store::CommandAdmission,
    wire::{publish_task_event, send_control_error, send_error, send_reply, task_sender},
};

pub(crate) async fn handle_surface_operation(
    state: Arc<CoordinatorState>,
    outgoing: &mpsc::Sender<ServerMessage>,
    connection_cancelled: &CancellationToken,
    session: SurfaceSession,
    request_id: u64,
    operation: SurfaceOperation,
) {
    let principal_id = session.principal_id;
    match operation {
        SurfaceOperation::ListDevices => {
            device_admin::list_devices(&state, outgoing, request_id, &session).await;
        }
        SurfaceOperation::RevokeDevice { device_id } => {
            device_admin::revoke_device(&state, outgoing, request_id, &session, device_id).await;
        }
        SurfaceOperation::ListTasks => {
            let result = state
                .store
                .list_tasks(principal_id)
                .await
                .map(|tasks| ServerMessage::TaskList { request_id, tasks });
            send_reply(outgoing, request_id, result).await;
        }
        SurfaceOperation::ListTargets => {
            let result = crate::task_opening::list_targets(&state, principal_id)
                .await
                .map(|targets| ServerMessage::TargetList {
                    request_id,
                    targets,
                });
            send_reply(outgoing, request_id, result).await;
        }
        SurfaceOperation::OpenTask {
            task_id,
            node_id,
            target,
        } => {
            let task = TaskSpec {
                task_id,
                principal_id,
                node_id,
                target,
            };
            let result = crate::task_opening::open_task(&state, task)
                .await
                .map(|()| ServerMessage::TaskOpened {
                    request_id,
                    task_id,
                });
            send_reply(outgoing, request_id, result).await;
        }
        SurfaceOperation::Attach {
            task_id,
            after_sequence,
        } => {
            if let Err(error) = attach_surface(
                state,
                outgoing.clone(),
                connection_cancelled.child_token(),
                request_id,
                task_id,
                after_sequence,
                principal_id,
            )
            .await
            {
                send_control_error(outgoing, Some(request_id), &error).await;
            }
        }
        SurfaceOperation::Submit {
            task_id,
            command_id,
            input,
        } => {
            let result = submit_command(
                &state,
                outgoing,
                request_id,
                task_id,
                command_id,
                input,
                principal_id,
                session.surface,
            )
            .await;
            if let Err(error) = result {
                send_control_error(outgoing, Some(request_id), &error).await;
            }
        }
        SurfaceOperation::DeleteTask { task_id } => {
            let result = crate::task_deletion::delete_task(&state, principal_id, task_id)
                .await
                .map(|()| ServerMessage::TaskDeleted {
                    request_id,
                    task_id,
                });
            send_reply(outgoing, request_id, result).await;
        }
    }
}

pub(crate) async fn attach_surface(
    state: Arc<CoordinatorState>,
    outgoing: mpsc::Sender<ServerMessage>,
    cancelled: CancellationToken,
    request_id: u64,
    task_id: TaskId,
    after_sequence: Option<u64>,
    principal_id: PrincipalId,
) -> Result<(), ControlError> {
    // Reject unknown tasks before allocating their long-lived broadcaster. The
    // suffix is read after subscription to preserve the replay-to-live boundary.
    state
        .store
        .load_task_for_principal(task_id, principal_id)
        .await?;
    let mut live = task_sender(&state, task_id).await.subscribe();
    let suffix = state
        .store
        .load_suffix(task_id, principal_id, after_sequence)
        .await?;
    outgoing
        .send(ServerMessage::Attached {
            request_id,
            task_id,
            through_sequence: suffix.through_sequence,
        })
        .await
        .map_err(|_| ControlError::invalid("surface disconnected during attachment"))?;
    for event in suffix.events {
        outgoing
            .send(ServerMessage::TaskEvent { event })
            .await
            .map_err(|_| ControlError::invalid("surface disconnected during replay"))?;
    }
    let through_sequence = suffix.through_sequence;
    tokio::spawn(async move {
        loop {
            tokio::select! {
                () = cancelled.cancelled() => return,
                event = live.recv() => match event {
                    Ok(event) if through_sequence.is_none_or(|sequence| event.sequence > sequence) => {
                        if outgoing.send(ServerMessage::TaskEvent { event }).await.is_err() {
                            return;
                        }
                    }
                    Ok(_) => {}
                    Err(broadcast::error::RecvError::Lagged(_)) => {
                        send_error(
                            &outgoing,
                            None,
                            ErrorCode::ReplayRequired,
                            "surface fell behind; reconnect with its last task sequence",
                        )
                        .await;
                        return;
                    }
                    Err(broadcast::error::RecvError::Closed) => return,
                }
            }
        }
    });
    Ok(())
}

#[allow(
    clippy::too_many_arguments,
    reason = "these values form the complete authenticated command admission boundary"
)]
async fn submit_command(
    state: &CoordinatorState,
    outgoing: &mpsc::Sender<ServerMessage>,
    request_id: u64,
    task_id: TaskId,
    command_id: CommandId,
    input: CommandInput,
    principal_id: PrincipalId,
    surface: SurfaceRef,
) -> Result<(), ControlError> {
    let task = state
        .store
        .load_task_for_principal(task_id, principal_id)
        .await?;
    let lifecycle = state.connection_lifecycle.lock().await;
    let node = state.nodes.lock().await.get(&task.node_id).cloned();
    let admission = state
        .store
        .admit_command(
            task_id,
            principal_id,
            surface,
            command_id,
            input,
            node.is_some(),
        )
        .await?;
    drop(lifecycle);
    let (command, event, pending) = match admission {
        CommandAdmission::NotAdmitted => {
            control_log::event(
                "warn",
                "command_rejected_node_offline",
                &serde_json::json!({
                    "task_id": task_id,
                    "command_id": command_id,
                    "node_id": task.node_id,
                }),
            );
            return Err(ControlError::node_offline());
        }
        CommandAdmission::Admitted { command, event } => (command, Some(*event), true),
        CommandAdmission::Existing { command, pending } => (command, None, pending),
    };
    control_log::event(
        "info",
        if event.is_some() {
            "command_admitted"
        } else {
            "command_admission_replayed"
        },
        &serde_json::json!({
            "task_id": task_id,
            "command_id": command.command_id,
            "node_id": task.node_id,
            "dispatched": pending && node.is_some(),
        }),
    );
    let _ = outgoing
        .send(ServerMessage::CommandAccepted {
            request_id,
            command_id: command.command_id,
        })
        .await;
    if let Some(event) = event {
        publish_task_event(state, event).await;
    }
    if let (true, Some(node)) = (pending, node) {
        let _ = node
            .outgoing
            .send(ServerMessage::Execute { task_id, command })
            .await;
    }
    Ok(())
}
