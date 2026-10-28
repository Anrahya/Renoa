//! What a connection may do to its principal's signed-in apps, and ending a
//! device's sessions once it is revoked.

use std::sync::Arc;

use renoa_protocol::{PrincipalId, SurfaceRef};
use tokio::sync::{Semaphore, mpsc};

use crate::{
    DeviceId, ErrorCode, ServerMessage, control_log,
    coordinator::CoordinatorState,
    wire::{send_control_error, send_error, send_reply},
};

/// How a surface connection proved itself. It decides whether the connection
/// may administer devices.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Authority {
    /// A browser connection ticket, issued after a passkey or a locally
    /// issued pairing code.
    Ticket,
    /// A device that proved possession of its registered key.
    KeyedDevice(DeviceId),
    /// A device that presented only its bearer credential, such as a chat
    /// surface's service link.
    BearerDevice(DeviceId),
}

impl Authority {
    pub(crate) const fn device_id(self) -> Option<DeviceId> {
        match self {
            Self::Ticket => None,
            Self::KeyedDevice(device_id) | Self::BearerDevice(device_id) => Some(device_id),
        }
    }

    /// Whether a person proved themselves on this connection, with a passkey
    /// or a device key. Only such a connection may list or revoke devices or
    /// control the Host, so a thin chat surface can do neither.
    pub(crate) const fn is_person_bound(self) -> bool {
        match self {
            Self::Ticket | Self::KeyedDevice(_) => true,
            Self::BearerDevice(_) => false,
        }
    }
}

/// The identity a surface connection operates under.
pub(crate) struct SurfaceSession {
    pub(crate) principal_id: PrincipalId,
    pub(crate) surface: SurfaceRef,
    pub(crate) authority: Authority,
    /// The connection's slots for Host calls waiting on an answer.
    pub(crate) host_calls: Arc<Semaphore>,
}

pub(crate) async fn list_devices(
    state: &CoordinatorState,
    outgoing: &mpsc::Sender<ServerMessage>,
    request_id: u64,
    session: &SurfaceSession,
) {
    if !allowed(outgoing, request_id, session.authority).await {
        return;
    }
    let result = state
        .store
        .list_devices(session.principal_id, session.authority.device_id())
        .await
        .map(|devices| ServerMessage::DeviceList {
            request_id,
            devices,
        });
    send_reply(outgoing, request_id, result).await;
}

pub(crate) async fn revoke_device(
    state: &CoordinatorState,
    outgoing: &mpsc::Sender<ServerMessage>,
    request_id: u64,
    session: &SurfaceSession,
    device_id: DeviceId,
) {
    if !allowed(outgoing, request_id, session.authority).await {
        return;
    }
    match state
        .store
        .revoke_owned_device(session.principal_id, device_id)
        .await
    {
        Ok(newly_revoked) => {
            if newly_revoked {
                control_log::event(
                    "info",
                    "device_revoked",
                    &serde_json::json!({
                        "device_id": device_id,
                        "principal_id": session.principal_id,
                        "by_device_id": session.authority.device_id(),
                    }),
                );
            }
            // The revocation is committed. Acknowledge it before the sessions
            // end, so a device that revokes itself still hears back.
            let _ = outgoing
                .send(ServerMessage::DeviceRevoked {
                    request_id,
                    device_id,
                })
                .await;
            end_device_sessions(state, device_id).await;
        }
        Err(error) => send_control_error(outgoing, Some(request_id), &error).await,
    }
}

async fn allowed(
    outgoing: &mpsc::Sender<ServerMessage>,
    request_id: u64,
    authority: Authority,
) -> bool {
    if authority.is_person_bound() {
        return true;
    }
    send_error(
        outgoing,
        Some(request_id),
        ErrorCode::InvalidRole,
        "devices are managed from a signed-in app or a browser; this connection is neither",
    )
    .await;
    false
}

/// Ends every live connection of a revoked device, under the lifecycle lock
/// that keeps an in-flight authentication from outliving the revocation.
pub(crate) async fn end_device_sessions(state: &CoordinatorState, device_id: DeviceId) {
    let _lifecycle = state.connection_lifecycle.lock().await;
    let sessions = state.sessions.lock().await.remove(&device_id);
    for session in sessions.into_iter().flatten().map(|(_, session)| session) {
        session.cancel();
    }
    state
        .nodes
        .lock()
        .await
        .retain(|_, node| node.device_id != device_id);
}
