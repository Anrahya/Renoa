use std::{sync::Arc, time::Duration};

use axum::{
    extract::{State, WebSocketUpgrade, ws::Message},
    http::StatusCode,
    response::{IntoResponse, Response},
};
use futures_util::{SinkExt, StreamExt};
use tokio::{sync::mpsc, time::timeout};
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use crate::{
    ClientMessage, DeviceId, ErrorCode, JSON_WS_VERSION, PeerIdentity, ServerMessage,
    coordinator::{CoordinatorState, NodeConnection, handle_surface_operation},
    identity_store::AuthenticatedDevice,
    json_ws::JsonOperation,
    node_messages::handle_node_operation,
    wire::{cleanup_connection, parse_message, send_control_error, send_error},
};

const OUTBOUND_CAPACITY: usize = 128;
const AUTHENTICATION_DEADLINE: Duration = Duration::from_secs(10);
const MAX_APPLICATION_MESSAGE_BYTES: usize = 1024 * 1024;

pub(crate) async fn upgrade_connection(
    State(state): State<Arc<CoordinatorState>>,
    upgrade: WebSocketUpgrade,
) -> Response {
    let Ok(slot) = Arc::clone(&state.connection_slots).try_acquire_owned() else {
        return StatusCode::SERVICE_UNAVAILABLE.into_response();
    };
    upgrade
        .max_message_size(MAX_APPLICATION_MESSAGE_BYTES)
        .max_frame_size(MAX_APPLICATION_MESSAGE_BYTES)
        .on_upgrade(move |socket| async move {
            serve_connection(state, socket).await;
            drop(slot);
        })
}

async fn serve_connection(state: Arc<CoordinatorState>, socket: axum::extract::ws::WebSocket) {
    let connection_id = Uuid::new_v4();
    let (wire_sender, mut wire_receiver) = socket.split();
    let (outgoing, outbound) = mpsc::channel::<ServerMessage>(OUTBOUND_CAPACITY);
    let connection_cancelled = CancellationToken::new();
    let writer = spawn_writer(wire_sender, outbound, connection_cancelled.clone());
    let Ok(Some(device)) = timeout(
        AUTHENTICATION_DEADLINE,
        read_peer(&state, &mut wire_receiver, &outgoing),
    )
    .await
    else {
        drop(outgoing);
        let _ = writer.await;
        connection_cancelled.cancel();
        return;
    };
    let device_id = device.device_id;
    let peer = device.peer;
    if !activate_device(
        &state,
        device_id,
        &peer,
        connection_id,
        &outgoing,
        &connection_cancelled,
    )
    .await
    {
        connection_cancelled.cancel();
        drop(outgoing);
        let _ = writer.await;
        return;
    }
    serve_messages(
        Arc::clone(&state),
        &mut wire_receiver,
        &outgoing,
        &connection_cancelled,
        &peer,
        connection_id,
    )
    .await;

    cleanup_connection(&state, &peer, connection_id).await;
    cleanup_session(&state, device_id, connection_id).await;
    connection_cancelled.cancel();
    drop(outgoing);
    let _ = writer.await;
}

async fn activate_device(
    state: &CoordinatorState,
    device_id: DeviceId,
    peer: &PeerIdentity,
    connection_id: Uuid,
    outgoing: &mpsc::Sender<ServerMessage>,
    cancelled: &CancellationToken,
) -> bool {
    let lifecycle = state.connection_lifecycle.lock().await;
    let active = match state.store.device_is_active(device_id).await {
        Ok(active) => active,
        Err(error) => {
            send_control_error(outgoing, None, &error).await;
            return false;
        }
    };
    if !active {
        return false;
    }
    let pending = if let PeerIdentity::Node { node_id } = *peer {
        state.nodes.lock().await.remove(&node_id);
        match state.store.load_pending_executions(node_id).await {
            Ok(pending) => pending,
            Err(error) => {
                send_control_error(outgoing, None, &error).await;
                return false;
            }
        }
    } else {
        Vec::new()
    };
    register_session(state, device_id, connection_id, cancelled.clone()).await;
    if outgoing
        .send(ServerMessage::Authenticated {
            version: JSON_WS_VERSION,
        })
        .await
        .is_err()
    {
        cleanup_connection(state, peer, connection_id).await;
        cleanup_session(state, device_id, connection_id).await;
        return false;
    }
    for execution in pending {
        if outgoing
            .send(ServerMessage::Execute {
                task_id: execution.task_id,
                command: execution.command,
            })
            .await
            .is_err()
        {
            cleanup_session(state, device_id, connection_id).await;
            return false;
        }
    }
    if let PeerIdentity::Node { node_id } = *peer {
        state.nodes.lock().await.insert(
            node_id,
            NodeConnection {
                connection_id,
                device_id,
                outgoing: outgoing.clone(),
            },
        );
    }
    drop(lifecycle);
    true
}

async fn serve_messages(
    state: Arc<CoordinatorState>,
    receiver: &mut futures_util::stream::SplitStream<axum::extract::ws::WebSocket>,
    outgoing: &mpsc::Sender<ServerMessage>,
    cancelled: &CancellationToken,
    peer: &PeerIdentity,
    connection_id: Uuid,
) {
    loop {
        let message = tokio::select! {
            () = cancelled.cancelled() => break,
            message = receiver.next() => message,
        };
        let Some(message) = message else { break };
        let Ok(message) = message else { break };
        let Some(message) = parse_message(message) else {
            send_error(
                outgoing,
                None,
                ErrorCode::InvalidMessage,
                "message is not valid Renoa JSON",
            )
            .await;
            continue;
        };
        let Some(operation) = message.into_operation() else {
            send_error(
                outgoing,
                None,
                ErrorCode::InvalidMessage,
                "identity may only be established once",
            )
            .await;
            continue;
        };
        match (peer, operation) {
            (
                PeerIdentity::Surface {
                    principal_id,
                    surface,
                },
                JsonOperation::Surface {
                    request_id,
                    operation,
                },
            ) => {
                handle_surface_operation(
                    Arc::clone(&state),
                    outgoing,
                    cancelled,
                    *principal_id,
                    surface.clone(),
                    request_id,
                    operation,
                )
                .await;
            }
            (PeerIdentity::Node { node_id }, JsonOperation::Node(operation)) => {
                handle_node_operation(&state, outgoing, *node_id, connection_id, operation).await;
            }
            (PeerIdentity::Surface { .. }, JsonOperation::Node(_)) => {
                send_error(
                    outgoing,
                    None,
                    ErrorCode::InvalidRole,
                    "surface cannot issue node operations",
                )
                .await;
            }
            (PeerIdentity::Node { .. }, JsonOperation::Surface { request_id, .. }) => {
                send_error(
                    outgoing,
                    Some(request_id),
                    ErrorCode::InvalidRole,
                    "node cannot issue surface operations",
                )
                .await;
            }
        }
    }
}

fn spawn_writer(
    mut wire_sender: futures_util::stream::SplitSink<axum::extract::ws::WebSocket, Message>,
    mut outbound: mpsc::Receiver<ServerMessage>,
    cancelled: CancellationToken,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        loop {
            tokio::select! {
                () = cancelled.cancelled() => break,
                message = outbound.recv() => {
                    let Some(message) = message else { break };
                    if !message.has_interoperable_numbers() {
                        let error = ServerMessage::Error {
                            request_id: None,
                            code: ErrorCode::Internal,
                            message: "internal coordinator error".to_owned(),
                        };
                        let Ok(json) = serde_json::to_string(&error) else { break };
                        let _ = wire_sender.send(Message::Text(json.into())).await;
                        break;
                    }
                    let Ok(json) = serde_json::to_string(&message) else { break };
                    if wire_sender.send(Message::Text(json.into())).await.is_err() {
                        break;
                    }
                }
            }
        }
        cancelled.cancel();
    })
}

async fn read_peer(
    state: &CoordinatorState,
    receiver: &mut futures_util::stream::SplitStream<axum::extract::ws::WebSocket>,
    outgoing: &mpsc::Sender<ServerMessage>,
) -> Option<AuthenticatedDevice> {
    let Some(Ok(message)) = receiver.next().await else {
        return None;
    };
    let Some(message) = parse_message(message) else {
        send_error(
            outgoing,
            None,
            ErrorCode::AuthenticationFailed,
            "authentication failed",
        )
        .await;
        return None;
    };
    let device = match message {
        ClientMessage::Enroll { version, token } => {
            if !check_version(outgoing, version).await {
                return None;
            }
            match state.store.claim_enrollment(token).await {
                Ok(credentials) => {
                    let _ = outgoing
                        .send(ServerMessage::Enrolled {
                            version: JSON_WS_VERSION,
                            credentials,
                        })
                        .await;
                }
                Err(error) => send_control_error(outgoing, None, &error).await,
            }
            return None;
        }
        ClientMessage::Authenticate {
            version,
            credentials,
        } => {
            if !check_version(outgoing, version).await {
                return None;
            }
            match state.store.authenticate_device(credentials).await {
                Ok(device) => device,
                Err(error) => {
                    send_control_error(outgoing, None, &error).await;
                    return None;
                }
            }
        }
        ClientMessage::ListTasks { .. }
        | ClientMessage::Attach { .. }
        | ClientMessage::Submit { .. }
        | ClientMessage::AcknowledgeExecution { .. }
        | ClientMessage::PublishExecutionEvents { .. } => {
            send_error(
                outgoing,
                None,
                ErrorCode::InvalidMessage,
                "the first message must establish identity",
            )
            .await;
            return None;
        }
    };
    Some(device)
}

async fn register_session(
    state: &CoordinatorState,
    device_id: DeviceId,
    connection_id: Uuid,
    cancelled: CancellationToken,
) {
    state
        .sessions
        .lock()
        .await
        .entry(device_id)
        .or_default()
        .insert(connection_id, cancelled);
}

async fn cleanup_session(state: &CoordinatorState, device_id: DeviceId, connection_id: Uuid) {
    let mut sessions = state.sessions.lock().await;
    let Some(device_sessions) = sessions.get_mut(&device_id) else {
        return;
    };
    device_sessions.remove(&connection_id);
    if device_sessions.is_empty() {
        sessions.remove(&device_id);
    }
}

async fn check_version(outgoing: &mpsc::Sender<ServerMessage>, version: u32) -> bool {
    if version != JSON_WS_VERSION {
        send_error(
            outgoing,
            None,
            ErrorCode::VersionMismatch,
            format!("unsupported protocol version {version}"),
        )
        .await;
        return false;
    }
    true
}
