//! Which authenticated connections are live: activating a device or browser
//! peer after it authenticates, and forgetting it when it ends.

use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use crate::{
    DeviceId, JSON_WS_VERSION, PeerIdentity, ServerMessage,
    coordinator::{CoordinatorState, NodeConnection},
    wire::{cleanup_connection, send_control_error},
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

pub(crate) async fn activate_device(
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
