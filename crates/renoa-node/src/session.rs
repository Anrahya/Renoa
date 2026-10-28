use std::{
    collections::HashMap,
    collections::HashSet,
    sync::Arc,
    time::{Duration, Instant},
};

use futures_util::{SinkExt, StreamExt};
use renoa_control::{ClientMessage, DeviceCredentials, JSON_WS_VERSION, ServerMessage, TaskId};
use renoa_protocol::{CommandEnvelope, CommandId, TargetRef};
use tokio::{
    sync::{broadcast, broadcast::error::RecvError, watch},
    task::JoinSet,
};
use tokio_tungstenite::{
    MaybeTlsStream, WebSocketStream, connect_async_with_config,
    tungstenite::{Message, protocol::WebSocketConfig},
};
use tokio_util::sync::CancellationToken;

use crate::{
    bridge::{ExecutionTask, NodeError, NodeRuntime, finish_execution, schedule_pending},
    live::LiveFrame,
    node_log,
};
use publication::{Publication, refresh_publications, send_live, send_pending};

type Socket = WebSocketStream<MaybeTlsStream<tokio::net::TcpStream>>;
const MAX_APPLICATION_MESSAGE_BYTES: usize = 1024 * 1024;
const AGENT_REFRESH_INTERVAL: Duration = Duration::from_secs(5);

pub(crate) enum SessionEnd {
    Disconnected {
        reason: String,
        connected_for: Option<Duration>,
    },
    Shutdown,
}

#[allow(
    clippy::too_many_arguments,
    reason = "these values are the complete state of one reconnectable node session"
)]
pub(crate) async fn serve_session(
    endpoint: &str,
    credentials: &DeviceCredentials,
    runtime: Arc<NodeRuntime>,
    shutdown: &CancellationToken,
    commits: &mut watch::Receiver<u64>,
    tasks: &mut JoinSet<ExecutionTask>,
    running: &mut HashSet<TaskId>,
) -> Result<SessionEnd, NodeError> {
    let mut authenticated_at = None;
    let result = serve_session_inner(
        endpoint,
        credentials,
        runtime,
        shutdown,
        commits,
        tasks,
        running,
        &mut authenticated_at,
    )
    .await;
    match result {
        Err(NodeError::Transport(reason)) => Ok(SessionEnd::Disconnected {
            reason,
            connected_for: authenticated_at.map(|started: Instant| started.elapsed()),
        }),
        result => result,
    }
}

#[allow(
    clippy::too_many_arguments,
    reason = "these values are the complete state of one reconnectable node session"
)]
async fn serve_session_inner(
    endpoint: &str,
    credentials: &DeviceCredentials,
    runtime: Arc<NodeRuntime>,
    shutdown: &CancellationToken,
    commits: &mut watch::Receiver<u64>,
    tasks: &mut JoinSet<ExecutionTask>,
    running: &mut HashSet<TaskId>,
    authenticated_at: &mut Option<Instant>,
) -> Result<SessionEnd, NodeError> {
    let websocket = WebSocketConfig::default()
        .max_message_size(Some(MAX_APPLICATION_MESSAGE_BYTES))
        .max_frame_size(Some(MAX_APPLICATION_MESSAGE_BYTES));
    let (mut socket, _) = connect_async_with_config(endpoint, Some(websocket), false)
        .await
        .map_err(|error| NodeError::Transport(error.to_string()))?;
    send_client(
        &mut socket,
        &ClientMessage::Authenticate {
            version: JSON_WS_VERSION,
            credentials: credentials.clone(),
        },
    )
    .await?;
    match receive_server(&mut socket).await? {
        ServerMessage::Authenticated { version } if version == JSON_WS_VERSION => {}
        ServerMessage::Error { code, message, .. } => {
            return Err(NodeError::Rejected { code, message });
        }
        _ => {
            return Err(NodeError::Protocol(
                "coordinator did not authenticate the node".to_owned(),
            ));
        }
    }
    *authenticated_at = Some(Instant::now());
    node_log::event("info", "coordinator_connected", &serde_json::json!({}));
    let mut advertised = None;
    advertise(&mut socket, &runtime, &mut advertised).await?;
    let mut agent_refresh = tokio::time::interval(AGENT_REFRESH_INTERVAL);
    agent_refresh.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);

    let mut live = runtime.live.subscribe();
    let mut publications = HashMap::new();
    refresh_publications(&runtime, &mut publications).await?;
    send_pending(&mut socket, &runtime.state, &mut live, &mut publications).await?;

    loop {
        tokio::select! {
            // Polled in order, so a queued frame never waits behind a commit.
            // The order of frames against records does not rest on this:
            // `send_pending` sends the frames already queued before the
            // records it has read.
            biased;
            () = shutdown.cancelled() => return Ok(SessionEnd::Shutdown),
            frame = live.recv() => match frame {
                Ok(frame) => send_live(&mut socket, frame).await?,
                // Dropped frames are the surface's gap to notice; nothing is replayed.
                Err(RecvError::Lagged(_)) => {}
                Err(RecvError::Closed) => {
                    return Err(NodeError::Protocol("local live channel closed".to_owned()));
                }
            },
            changed = commits.changed() => {
                if changed.is_err() {
                    return Err(NodeError::Protocol("local commit signal closed".to_owned()));
                }
                refresh_publications(&runtime, &mut publications).await?;
                send_pending(&mut socket, &runtime.state, &mut live, &mut publications).await?;
            }
            completed = tasks.join_next(), if !tasks.is_empty() => {
                if let Some(completed) = completed {
                    finish_execution(completed, running)?;
                    schedule_pending(Arc::clone(&runtime), tasks, running).await?;
                }
            }
            _ = agent_refresh.tick() => advertise(&mut socket, &runtime, &mut advertised).await?,
            message = receive_server(&mut socket) => {
                let message = message?;
                handle_server_message(
                    &mut socket,
                    &runtime,
                    tasks,
                    running,
                    &mut publications,
                    &mut live,
                    message,
                ).await?;
            }
        }
    }
}

/// Advertises the Host's agents when they differ from this connection's last
/// advertisement. The Host has no change notification, so the session polls.
async fn advertise(
    socket: &mut Socket,
    runtime: &NodeRuntime,
    advertised: &mut Option<Vec<TargetRef>>,
) -> Result<(), NodeError> {
    let targets = match runtime.advertised_targets().await {
        Ok(targets) => targets,
        Err(error) => {
            // Existing tasks keep executing; the next poll retries the listing.
            node_log::event(
                "warn",
                "targets_unavailable",
                &serde_json::json!({ "error": error.to_string() }),
            );
            return Ok(());
        }
    };
    if advertised.as_ref() == Some(&targets) {
        return Ok(());
    }
    send_client(
        socket,
        &ClientMessage::AdvertiseTargets {
            targets: targets.clone(),
        },
    )
    .await?;
    node_log::event(
        "info",
        "targets_advertised",
        &serde_json::json!({
            "targets": targets.iter().map(TargetRef::as_str).collect::<Vec<_>>(),
        }),
    );
    *advertised = Some(targets);
    Ok(())
}

#[allow(
    clippy::too_many_arguments,
    reason = "these values are the complete state of one authenticated node session"
)]
async fn handle_server_message(
    socket: &mut Socket,
    runtime: &Arc<NodeRuntime>,
    tasks: &mut JoinSet<ExecutionTask>,
    running: &mut HashSet<TaskId>,
    publications: &mut HashMap<CommandId, Publication>,
    live: &mut broadcast::Receiver<LiveFrame>,
    message: ServerMessage,
) -> Result<(), NodeError> {
    match message {
        ServerMessage::Execute { task_id, command } => {
            handle_execute(runtime, tasks, running, task_id, command).await?;
            refresh_publications(runtime, publications).await?;
            send_pending(socket, &runtime.state, live, publications).await?;
        }
        ServerMessage::ExecutionAcknowledged { command_id } => {
            let publication = publications.get_mut(&command_id).ok_or_else(|| {
                NodeError::Protocol(format!(
                    "received admission acknowledgement for unknown command {command_id}"
                ))
            })?;
            if !publication.admission_in_flight {
                return Err(NodeError::Protocol(format!(
                    "received unsolicited admission acknowledgement for command {command_id}"
                )));
            }
            runtime.state.acknowledge_admission(command_id).await?;
            publication.record.admission_acked = true;
            publication.admission_in_flight = false;
            refresh_publications(runtime, publications).await?;
        }
        ServerMessage::ExecutionEventsAccepted {
            command_id,
            through_execution_sequence,
        } => {
            let publication = publications.get_mut(&command_id).ok_or_else(|| {
                NodeError::Protocol(format!(
                    "received event acknowledgement for unknown command {command_id}"
                ))
            })?;
            let expected = publication.event_in_flight.ok_or_else(|| {
                NodeError::Protocol(format!(
                    "received unsolicited event acknowledgement for command {command_id}"
                ))
            })?;
            if through_execution_sequence != expected {
                return Err(NodeError::Protocol(format!(
                    "event acknowledgement for command {command_id} ended at sequence \
                     {through_execution_sequence}, expected {expected}"
                )));
            }
            runtime
                .state
                .advance_publication(command_id, through_execution_sequence)
                .await?;
            publication.record.published_through = Some(through_execution_sequence);
            publication.event_in_flight = None;
            refresh_publications(runtime, publications).await?;
            send_pending(socket, &runtime.state, live, publications).await?;
        }
        ServerMessage::Stop {
            task_id,
            command_id,
        } => {
            // Acknowledged only once the stop is durable here.
            runtime.stop(task_id, command_id).await?;
            send_client(
                socket,
                &ClientMessage::AcknowledgeStop {
                    task_id,
                    command_id,
                },
            )
            .await?;
        }
        ServerMessage::Error { code, message, .. } => {
            return Err(NodeError::Rejected { code, message });
        }
        ServerMessage::Authenticated { .. }
        | ServerMessage::DeviceChallenge { .. }
        | ServerMessage::DeviceList { .. }
        | ServerMessage::DeviceRevoked { .. }
        | ServerMessage::Enrolled { .. }
        | ServerMessage::TaskList { .. }
        | ServerMessage::TargetList { .. }
        | ServerMessage::TaskOpened { .. }
        | ServerMessage::TaskDeleted { .. }
        | ServerMessage::Attached { .. }
        | ServerMessage::CommandAccepted { .. }
        | ServerMessage::StopAccepted { .. }
        | ServerMessage::LiveActivity { .. }
        | ServerMessage::TaskEvent { .. } => {
            return Err(NodeError::Protocol(
                "coordinator sent a surface-only message to a node".to_owned(),
            ));
        }
    }
    Ok(())
}

async fn handle_execute(
    runtime: &Arc<NodeRuntime>,
    tasks: &mut JoinSet<ExecutionTask>,
    running: &mut HashSet<TaskId>,
    task_id: TaskId,
    command: CommandEnvelope,
) -> Result<(), NodeError> {
    let binding = runtime.proposed_binding(&command.target)?;
    let command_id = command.command_id;
    runtime.state.admit(task_id, command, binding).await?;
    runtime.state.require_admission_ack(command_id).await?;
    runtime.signal_commit();
    schedule_pending(Arc::clone(runtime), tasks, running).await?;
    Ok(())
}

async fn send_client(socket: &mut Socket, message: &ClientMessage) -> Result<(), NodeError> {
    let json = serde_json::to_string(message)?;
    socket
        .send(Message::Text(json.into()))
        .await
        .map_err(|error| NodeError::Transport(error.to_string()))
}

async fn receive_server(socket: &mut Socket) -> Result<ServerMessage, NodeError> {
    loop {
        match socket.next().await {
            Some(Ok(Message::Text(json))) => return Ok(serde_json::from_str(&json)?),
            Some(Ok(Message::Ping(payload))) => {
                if socket.send(Message::Pong(payload)).await.is_err() {
                    return Err(NodeError::Transport(
                        "connection closed while answering a ping".to_owned(),
                    ));
                }
            }
            Some(Ok(Message::Pong(_))) => {}
            Some(Ok(Message::Close(frame))) => {
                return Err(NodeError::Transport(match frame {
                    Some(frame) => format!("coordinator closed: {}", frame.reason),
                    None => "coordinator closed the WebSocket".to_owned(),
                }));
            }
            Some(Err(error)) => return Err(NodeError::Transport(error.to_string())),
            None => {
                return Err(NodeError::Transport(
                    "coordinator connection ended".to_owned(),
                ));
            }
            Some(Ok(Message::Binary(_) | Message::Frame(_))) => {
                return Err(NodeError::Protocol(
                    "coordinator sent a non-JSON WebSocket message".to_owned(),
                ));
            }
        }
    }
}

mod publication;
#[cfg(test)]
mod tests;
