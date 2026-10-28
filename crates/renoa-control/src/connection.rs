use std::{sync::Arc, time::Duration};

use axum::{
    extract::{State, WebSocketUpgrade, ws::Message},
    http::StatusCode,
    response::{IntoResponse, Response},
};
use futures_util::{SinkExt, StreamExt};
use tokio::{
    sync::mpsc,
    time::{Instant, MissedTickBehavior, interval_at, timeout},
};
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use crate::{
    ClientMessage, EnrollmentToken, ErrorCode, JSON_WS_VERSION, PeerIdentity, ServerMessage,
    coordinator::CoordinatorState,
    device_admin::{Authority, SurfaceSession},
    device_handshake::prove_possession,
    json_ws::JsonOperation,
    node_messages::handle_node_operation,
    session_registry::{DeviceLogin, activate_device, activate_ticket_peer, cleanup_session},
    surface_operations::handle_surface_operation,
    wire::{
        InboundMessage, classify_message, cleanup_connection, next_application_message,
        send_control_error, send_error,
    },
};

const OUTBOUND_CAPACITY: usize = 128;
/// How long a cancelled connection may spend sending what was already queued.
/// It bounds only that final flush: a write already blocked on a peer that is
/// not reading is not interrupted by it.
const CLOSE_FLUSH_DEADLINE: Duration = Duration::from_secs(2);
const AUTHENTICATION_DEADLINE: Duration = Duration::from_secs(10);
const TRANSPORT_KEEPALIVE_INTERVAL: Duration = Duration::from_secs(30);
const MAX_APPLICATION_MESSAGE_BYTES: usize = 1024 * 1024;

struct AuthenticatedPeer {
    authority: Authority,
    peer: PeerIdentity,
    /// Set for a device login: what activation re-checks under the lifecycle lock.
    device: Option<DeviceLogin>,
}

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
    let Ok(Some(authenticated)) = timeout(
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
    let AuthenticatedPeer {
        authority,
        peer,
        device,
    } = authenticated;
    let activated = match &device {
        Some(login) => {
            activate_device(
                &state,
                login,
                &peer,
                connection_id,
                &outgoing,
                &connection_cancelled,
            )
            .await
        }
        None => activate_ticket_peer(&peer, &outgoing).await,
    };
    if !activated {
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
        authority,
        connection_id,
    )
    .await;

    cleanup_connection(&state, &peer, connection_id).await;
    if let Some(login) = &device {
        cleanup_session(&state, login.device_id, connection_id).await;
    }
    connection_cancelled.cancel();
    drop(outgoing);
    let _ = writer.await;
}

async fn serve_messages(
    state: Arc<CoordinatorState>,
    receiver: &mut futures_util::stream::SplitStream<axum::extract::ws::WebSocket>,
    outgoing: &mpsc::Sender<ServerMessage>,
    cancelled: &CancellationToken,
    peer: &PeerIdentity,
    authority: Authority,
    connection_id: Uuid,
) {
    let host_calls = crate::host_control_rcp::call_slots();
    loop {
        let message = tokio::select! {
            () = cancelled.cancelled() => break,
            message = receiver.next() => message,
        };
        let message = match message {
            Some(Ok(message)) => match classify_message(message) {
                InboundMessage::Application(message) => message,
                InboundMessage::Control => continue,
                InboundMessage::Closed => break,
                InboundMessage::Invalid => {
                    send_error(
                        outgoing,
                        None,
                        ErrorCode::InvalidMessage,
                        "message is not valid Renoa JSON",
                    )
                    .await;
                    continue;
                }
            },
            Some(Err(_)) | None => break,
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
            (peer, JsonOperation::ListTargets { request_id }) => {
                crate::task_opening::handle_list_targets(
                    &state,
                    outgoing,
                    peer,
                    connection_id,
                    request_id,
                )
                .await;
            }
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
                    SurfaceSession {
                        principal_id: *principal_id,
                        surface: surface.clone(),
                        authority,
                        host_calls: Arc::clone(&host_calls),
                    },
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
        let mut keepalive = interval_at(
            Instant::now() + TRANSPORT_KEEPALIVE_INTERVAL,
            TRANSPORT_KEEPALIVE_INTERVAL,
        );
        keepalive.set_missed_tick_behavior(MissedTickBehavior::Delay);
        loop {
            tokio::select! {
                biased;
                () = cancelled.cancelled() => {
                    // Close the queue so nothing new enters, then send only
                    // what was queued before the cancel. A revocation's
                    // acknowledgement is queued before its sessions end, so
                    // it reaches the device that asked for it, and a busy
                    // task cannot keep a cancelled connection open.
                    outbound.close();
                    let _ = timeout(CLOSE_FLUSH_DEADLINE, async {
                        while let Ok(message) = outbound.try_recv() {
                            if !write_frame(&mut wire_sender, &message).await {
                                break;
                            }
                        }
                    })
                    .await;
                    break;
                }
                message = outbound.recv() => {
                    let Some(message) = message else { break };
                    if !write_frame(&mut wire_sender, &message).await {
                        break;
                    }
                }
                _ = keepalive.tick() => {
                    if wire_sender.send(Message::Ping(Vec::new().into())).await.is_err() {
                        break;
                    }
                }
            }
        }
        cancelled.cancel();
    })
}

/// Sends one frame. Returns whether the connection may continue: a frame that
/// cannot be represented interoperably is replaced by an internal error and
/// ends the connection.
async fn write_frame(
    wire_sender: &mut futures_util::stream::SplitSink<axum::extract::ws::WebSocket, Message>,
    message: &ServerMessage,
) -> bool {
    if !message.has_interoperable_numbers() {
        let error = ServerMessage::Error {
            request_id: None,
            code: ErrorCode::Internal,
            message: "internal coordinator error".to_owned(),
        };
        if let Ok(json) = serde_json::to_string(&error) {
            let _ = wire_sender.send(Message::Text(json.into())).await;
        }
        return false;
    }
    let Ok(json) = serde_json::to_string(message) else {
        return false;
    };
    wire_sender.send(Message::Text(json.into())).await.is_ok()
}

async fn read_peer(
    state: &CoordinatorState,
    receiver: &mut futures_util::stream::SplitStream<axum::extract::ws::WebSocket>,
    outgoing: &mpsc::Sender<ServerMessage>,
) -> Option<AuthenticatedPeer> {
    let message = next_application_message(receiver, outgoing).await?;
    let authenticated = match message {
        ClientMessage::Enroll { version, token } => {
            enroll(state, outgoing, version, token).await;
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
                Ok(device) => {
                    let authority = if let Some(key) = &device.key {
                        if !prove_possession(receiver, outgoing, device.device_id, key).await {
                            return None;
                        }
                        Authority::KeyedDevice(device.device_id)
                    } else {
                        Authority::BearerDevice(device.device_id)
                    };
                    AuthenticatedPeer {
                        authority,
                        peer: device.peer,
                        device: Some(DeviceLogin {
                            device_id: device.device_id,
                            credential_hash: device.credential_hash,
                        }),
                    }
                }
                Err(error) => {
                    send_control_error(outgoing, None, &error).await;
                    return None;
                }
            }
        }
        ClientMessage::AuthenticateTicket { version, ticket } => {
            if !check_version(outgoing, version).await {
                return None;
            }
            match state
                .store
                .claim_connection_ticket(ticket, std::time::SystemTime::now())
                .await
            {
                Ok(peer) => AuthenticatedPeer {
                    authority: Authority::Ticket,
                    peer,
                    device: None,
                },
                Err(error) => {
                    send_control_error(outgoing, None, &error).await;
                    return None;
                }
            }
        }
        ClientMessage::DeviceProof { .. }
        | ClientMessage::ListTasks { .. }
        | ClientMessage::ListTargets { .. }
        | ClientMessage::ListDevices { .. }
        | ClientMessage::RevokeDevice { .. }
        | ClientMessage::OpenTask { .. }
        | ClientMessage::AdvertiseTargets { .. }
        | ClientMessage::Attach { .. }
        | ClientMessage::Submit { .. }
        | ClientMessage::StopCommand { .. }
        | ClientMessage::DeleteTask { .. }
        | ClientMessage::CallHost { .. }
        | ClientMessage::AcknowledgeExecution { .. }
        | ClientMessage::AcknowledgeStop { .. }
        | ClientMessage::PublishLiveActivity { .. }
        | ClientMessage::PublishExecutionEvents { .. }
        | ClientMessage::ServeHostControl
        | ClientMessage::CompleteHostCall { .. }
        | ClientMessage::SendAgentMessage { .. } => {
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
    Some(authenticated)
}

/// Answers an enrollment. The connection ends either way: an enrolled device
/// connects again with its credentials.
async fn enroll(
    state: &CoordinatorState,
    outgoing: &mpsc::Sender<ServerMessage>,
    version: u32,
    token: EnrollmentToken,
) {
    if !check_version(outgoing, version).await {
        return;
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
