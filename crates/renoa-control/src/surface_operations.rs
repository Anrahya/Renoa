//! Operations an authenticated surface performs on its principal's tasks.

use std::sync::Arc;

use renoa_protocol::{CommandId, CommandInput, PrincipalId, SurfaceRef};
use tokio::sync::{broadcast, mpsc};
use tokio_util::sync::CancellationToken;

use crate::{
    ControlError, ErrorCode, ServerMessage, TaskId, TaskSpec, control_log,
    coordinator::CoordinatorState,
    live_activity,
    operations::SurfaceOperation,
    store::CommandAdmission,
    wire::{publish_task_event, send_control_error, send_error, task_sender},
};

pub(crate) async fn handle_surface_operation(
    state: Arc<CoordinatorState>,
    outgoing: &mpsc::Sender<ServerMessage>,
    connection_cancelled: &CancellationToken,
    principal_id: PrincipalId,
    surface: SurfaceRef,
    request_id: u64,
    operation: SurfaceOperation,
) {
    match operation {
        SurfaceOperation::ListTasks => {
            serve_list_tasks(&state, outgoing, principal_id, request_id).await;
        }
        SurfaceOperation::ListTargets => {
            serve_list_targets(&state, outgoing, principal_id, request_id).await;
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
            serve_open_task(&state, outgoing, request_id, task).await;
        }
        SurfaceOperation::Attach {
            task_id,
            after_sequence,
            live,
        } => {
            let attached = attach_surface(
                state,
                outgoing.clone(),
                connection_cancelled.child_token(),
                request_id,
                task_id,
                after_sequence,
                principal_id,
                live,
            )
            .await;
            report(outgoing, request_id, attached).await;
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
                surface,
            )
            .await;
            report(outgoing, request_id, result).await;
        }
        SurfaceOperation::StopCommand {
            task_id,
            command_id,
        } => {
            let stopped =
                crate::command_stop::stop_command(&state, principal_id, task_id, command_id).await;
            answer(outgoing, request_id, stopped, |outcome| {
                ServerMessage::StopAccepted {
                    request_id,
                    command_id,
                    outcome,
                }
            })
            .await;
        }
        SurfaceOperation::DeleteTask { task_id } => {
            serve_delete_task(&state, outgoing, principal_id, request_id, task_id).await;
        }
    }
}

async fn serve_list_tasks(
    state: &CoordinatorState,
    outgoing: &mpsc::Sender<ServerMessage>,
    principal_id: PrincipalId,
    request_id: u64,
) {
    let tasks = state.store.list_tasks(principal_id).await;
    answer(outgoing, request_id, tasks, |tasks| {
        ServerMessage::TaskList { request_id, tasks }
    })
    .await;
}

async fn serve_list_targets(
    state: &CoordinatorState,
    outgoing: &mpsc::Sender<ServerMessage>,
    principal_id: PrincipalId,
    request_id: u64,
) {
    let targets = crate::task_opening::list_targets(state, principal_id).await;
    answer(outgoing, request_id, targets, |targets| {
        ServerMessage::TargetList {
            request_id,
            targets,
        }
    })
    .await;
}

async fn serve_open_task(
    state: &CoordinatorState,
    outgoing: &mpsc::Sender<ServerMessage>,
    request_id: u64,
    task: TaskSpec,
) {
    let task_id = task.task_id;
    let opened = crate::task_opening::open_task(state, task).await;
    answer(outgoing, request_id, opened, |()| {
        ServerMessage::TaskOpened {
            request_id,
            task_id,
        }
    })
    .await;
}

async fn serve_delete_task(
    state: &CoordinatorState,
    outgoing: &mpsc::Sender<ServerMessage>,
    principal_id: PrincipalId,
    request_id: u64,
    task_id: TaskId,
) {
    let deleted = crate::task_deletion::delete_task(state, principal_id, task_id).await;
    answer(outgoing, request_id, deleted, |()| {
        ServerMessage::TaskDeleted {
            request_id,
            task_id,
        }
    })
    .await;
}

/// Sends the error that ended a request whose success answers for itself.
async fn report(
    outgoing: &mpsc::Sender<ServerMessage>,
    request_id: u64,
    result: Result<(), ControlError>,
) {
    if let Err(error) = result {
        send_control_error(outgoing, Some(request_id), &error).await;
    }
}

/// Sends a request's reply, or the error that ended it.
async fn answer<T>(
    outgoing: &mpsc::Sender<ServerMessage>,
    request_id: u64,
    result: Result<T, ControlError>,
    reply: impl FnOnce(T) -> ServerMessage,
) {
    match result {
        Ok(value) => {
            let _ = outgoing.send(reply(value)).await;
        }
        Err(error) => send_control_error(outgoing, Some(request_id), &error).await,
    }
}

#[allow(
    clippy::too_many_arguments,
    reason = "these values are the complete authenticated attachment request"
)]
pub(crate) async fn attach_surface(
    state: Arc<CoordinatorState>,
    outgoing: mpsc::Sender<ServerMessage>,
    cancelled: CancellationToken,
    request_id: u64,
    task_id: TaskId,
    after_sequence: Option<u64>,
    principal_id: PrincipalId,
    wants_live_activity: bool,
) -> Result<(), ControlError> {
    // Reject unknown tasks before allocating their long-lived broadcaster. The
    // suffix is read after subscription to preserve the replay-to-live boundary.
    let task = state
        .store
        .load_task_for_principal(task_id, principal_id)
        .await?;
    let mut live = task_sender(&state, task_id).await.subscribe();
    let suffix = state
        .store
        .load_suffix(task_id, principal_id, after_sequence)
        .await?;
    // Live frames are subscribed after the suffix is read. A frame published
    // before a replayed record was published before this point and is never
    // received, and every record committed from here on is forwarded live,
    // after the frames that preceded it.
    let mut live_frames = if wants_live_activity {
        Some(live_activity::subscribe(&state, task_id, task.node_id).await)
    } else {
        None
    };
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
                frame = live_activity::next_frame(&mut live_frames) => match frame {
                    // Live frames are best effort: a full queue drops a frame,
                    // and a receiver that fell behind skips ahead without
                    // asking the surface to replay. The surface sees the gap
                    // in the frames' sequence.
                    Ok(frame) => live_activity::forward(&outgoing, task_id, frame),
                    Err(broadcast::error::RecvError::Lagged(_)) => {}
                    Err(broadcast::error::RecvError::Closed) => live_frames = None,
                },
                event = live.recv() => match event {
                    Ok(event) if through_sequence.is_none_or(|sequence| event.sequence > sequence) => {
                        // Frames the node published before this record go first.
                        live_activity::forward_waiting(&mut live_frames, &outgoing, task_id);
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
