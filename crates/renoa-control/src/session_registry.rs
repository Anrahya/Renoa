//! Which authenticated connections are live: activating a device or browser
//! peer after it authenticates, and forgetting it when it ends.

use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use crate::{
    DeviceId, ErrorCode, JSON_WS_VERSION, PeerIdentity, ServerMessage,
    coordinator::{CoordinatorState, NodeConnection},
    wire::{cleanup_connection, send_control_error, send_error},
};

pub(crate) async fn activate_ticket_peer(
    peer: &PeerIdentity,
    outgoing: &mpsc::Sender<ServerMessage>,
) -> bool {
    if !matches!(peer, PeerIdentity::Surface { .. }) {
        return false;
    }
    outgoing
        .send(ServerMessage::Authenticated {
            version: JSON_WS_VERSION,
        })
        .await
        .is_ok()
}

/// A device that authenticated, and the credential it did so with.
pub(crate) struct DeviceLogin {
    pub(crate) device_id: DeviceId,
    pub(crate) credential_hash: [u8; 32],
}

/// Registers the connection of a device that authenticated. Under the
/// lifecycle lock it checks that the device is not revoked and that the
/// credential it connected with is still the device's: a revocation or a
/// repeat sign-in that committed while the connection was proving its key
/// would otherwise end only the sessions registered by then.
pub(crate) async fn activate_device(
    state: &CoordinatorState,
    login: &DeviceLogin,
    peer: &PeerIdentity,
    connection_id: Uuid,
    outgoing: &mpsc::Sender<ServerMessage>,
    cancelled: &CancellationToken,
) -> bool {
    let device_id = login.device_id;
    let lifecycle = state.connection_lifecycle.lock().await;
    let active = match state
        .store
        .device_accepts(device_id, login.credential_hash)
        .await
    {
        Ok(active) => active,
        Err(error) => {
            send_control_error(outgoing, None, &error).await;
            return false;
        }
    };
    if !active {
        send_error(
            outgoing,
            None,
            ErrorCode::AuthenticationFailed,
            "authentication failed",
        )
        .await;
        return false;
    }
    let (pending, pending_stops) = if let PeerIdentity::Node { node_id } = *peer {
        state.nodes.lock().await.remove(&node_id);
        let snapshot = tokio::try_join!(
            state.store.load_pending_executions(node_id),
            state.store.load_pending_stops(node_id),
        );
        match snapshot {
            Ok(snapshot) => snapshot,
            Err(error) => {
                send_control_error(outgoing, None, &error).await;
                return false;
            }
        }
    } else {
        (Vec::new(), Vec::new())
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
    // After the executions, so the node has admitted a command before it is
    // told to stop it.
    for stop in pending_stops {
        if outgoing
            .send(ServerMessage::Stop {
                task_id: stop.task_id,
                command_id: stop.command_id,
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
                targets: Vec::new(),
            },
        );
    }
    drop(lifecycle);
    true
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

pub(crate) async fn cleanup_session(
    state: &CoordinatorState,
    device_id: DeviceId,
    connection_id: Uuid,
) {
    let mut sessions = state.sessions.lock().await;
    let Some(device_sessions) = sessions.get_mut(&device_id) else {
        return;
    };
    device_sessions.remove(&connection_id);
    if device_sessions.is_empty() {
        sessions.remove(&device_id);
    }
}
