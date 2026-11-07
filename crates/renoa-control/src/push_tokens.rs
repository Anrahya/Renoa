//! A signed-in app's push token: registering and clearing it over RCP, and
//! the devices a wake goes to.
//!
//! A device holds at most one token and a token belongs to one device. The
//! token is opaque to RCP: it is stored to be sent to the push provider and is
//! never logged or echoed in an error.

use std::sync::Arc;

use renoa_protocol::PrincipalId;
use rusqlite::{Connection, OptionalExtension, TransactionBehavior, params};
use tokio::sync::mpsc;

use crate::{
    ControlError, DeviceId, ErrorCode, PeerIdentity, PushToken, ServerMessage, control_log,
    control_schema::open_connection,
    coordinator::CoordinatorState,
    device_admin::SurfaceSession,
    operations::PushOperation,
    store::{ControlStore, blocking, id_error, json_error, sqlite_error},
    wire::{send_control_error, send_error},
};

const MAX_TOKEN_BYTES: usize = 4096;

/// The push services a token may belong to. Only FCM exists; another
/// platform adds a variant, or never registers and keeps a socket.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PushProvider {
    Fcm,
}

impl PushProvider {
    const fn as_str(self) -> &'static str {
        match self {
            Self::Fcm => "fcm",
        }
    }
}

/// Checks a registration before anything is written. Neither error repeats
/// the token.
fn validate(provider: &str, token: &PushToken) -> Result<PushProvider, ControlError> {
    if provider != PushProvider::Fcm.as_str() {
        let named = if provider.len() <= 32 && provider.bytes().all(|byte| byte.is_ascii_graphic())
        {
            format!("provider {provider:?}")
        } else {
            "this provider".to_owned()
        };
        return Err(ControlError::invalid(format!(
            "{named} is not supported; expected \"fcm\""
        )));
    }
    let token = token.expose();
    let length = token.len();
    if length == 0 || length > MAX_TOKEN_BYTES || !token.bytes().all(|b| b.is_ascii_graphic()) {
        return Err(ControlError::invalid(format!(
            "token must be 1 to {MAX_TOKEN_BYTES} bytes of printable ASCII without spaces; \
             received {length} bytes"
        )));
    }
    Ok(PushProvider::Fcm)
}

/// What a registration changed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Registration {
    /// The device now holds the token, replacing any token it held before.
    Stored,
    /// The device already held exactly this token.
    Unchanged,
    /// Another device of the same principal held the token, and lost it.
    Moved { from: DeviceId },
}

/// A device a wake goes to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PushTarget {
    pub(crate) device_id: DeviceId,
    pub(crate) token: PushToken,
}

impl ControlStore {
    /// Registers `token` for `device_id` in one immediate transaction. The
    /// principal is the one the device was enrolled for.
    async fn set_push_token(
        &self,
        device_id: DeviceId,
        provider: PushProvider,
        token: PushToken,
    ) -> Result<(PrincipalId, Registration), ControlError> {
        let path = Arc::clone(&self.path);
        blocking(move || {
            let mut connection = open_connection(&path)?;
            let transaction = connection
                .transaction_with_behavior(TransactionBehavior::Immediate)
                .map_err(sqlite_error)?;
            let principal_id = active_device_principal(&transaction, device_id)?;
            let holder = transaction
                .query_row(
                    "SELECT device_id, principal_id FROM push_tokens WHERE token = ?1",
                    [token.expose()],
                    |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
                )
                .optional()
                .map_err(sqlite_error)?;
            let registration = match holder {
                Some((_, holder_principal)) if holder_principal != principal_id.to_string() => {
                    return Err(ControlError::conflict(
                        "token is registered to another principal; sign that device out first",
                    ));
                }
                Some((holder, _)) if holder == device_id.to_string() => Registration::Unchanged,
                Some((holder, _)) => {
                    transaction
                        .execute("DELETE FROM push_tokens WHERE device_id = ?1", [&holder])
                        .map_err(sqlite_error)?;
                    Registration::Moved {
                        from: DeviceId::from_uuid(holder.parse().map_err(id_error)?),
                    }
                }
                None => Registration::Stored,
            };
            transaction
                .execute(
                    "INSERT INTO push_tokens (device_id, principal_id, provider, token)
                     VALUES (?1, ?2, ?3, ?4)
                     ON CONFLICT (device_id) DO UPDATE
                     SET provider = excluded.provider, token = excluded.token",
                    params![
                        device_id.to_string(),
                        principal_id.to_string(),
                        provider.as_str(),
                        token.expose(),
                    ],
                )
                .map_err(sqlite_error)?;
            transaction.commit().map_err(sqlite_error)?;
            Ok((principal_id, registration))
        })
        .await
    }

    /// Removes the device's token. Returns whether it held one.
    async fn clear_push_token(&self, device_id: DeviceId) -> Result<bool, ControlError> {
        let path = Arc::clone(&self.path);
        blocking(move || {
            let connection = open_connection(&path)?;
            forget_device_token(&connection, device_id).map(|removed| removed > 0)
        })
        .await
    }

    /// The principal's devices that hold a token and are not revoked.
    pub(crate) async fn push_targets(
        &self,
        principal_id: PrincipalId,
    ) -> Result<Vec<PushTarget>, ControlError> {
        let path = Arc::clone(&self.path);
        blocking(move || {
            let connection = open_connection(&path)?;
            let mut statement = connection
                .prepare(
                    "SELECT t.device_id, t.token
                     FROM push_tokens t JOIN devices d ON d.device_id = t.device_id
                     WHERE t.principal_id = ?1 AND d.revoked = 0
                     ORDER BY t.device_id",
                )
                .map_err(sqlite_error)?;
            let rows = statement
                .query_map([principal_id.to_string()], |row| {
                    Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
                })
                .map_err(sqlite_error)?;
            let mut targets = Vec::new();
            for row in rows {
                let (device_id, token) = row.map_err(sqlite_error)?;
                targets.push(PushTarget {
                    device_id: DeviceId::from_uuid(device_id.parse().map_err(id_error)?),
                    token: PushToken::new(token),
                });
            }
            Ok(targets)
        })
        .await
    }

    /// Deletes a token the push provider called dead, only while the device
    /// still holds that same token: one it registered since is kept.
    pub(crate) async fn drop_push_token(&self, target: PushTarget) -> Result<bool, ControlError> {
        let path = Arc::clone(&self.path);
        blocking(move || {
            open_connection(&path)?
                .execute(
                    "DELETE FROM push_tokens WHERE device_id = ?1 AND token = ?2",
                    params![target.device_id.to_string(), target.token.expose()],
                )
                .map(|removed| removed > 0)
                .map_err(sqlite_error)
        })
        .await
    }
}

/// Deletes the device's token row. Revocation calls it in the transaction that
/// revokes the device, so a revoked device is never woken again.
pub(crate) fn forget_device_token(
    connection: &Connection,
    device_id: DeviceId,
) -> Result<usize, ControlError> {
    connection
        .execute(
            "DELETE FROM push_tokens WHERE device_id = ?1",
            [device_id.to_string()],
        )
        .map_err(sqlite_error)
}

/// The principal an active surface device was enrolled for. A revoked device
/// registers nothing, even from a connection that has not ended yet.
fn active_device_principal(
    connection: &Connection,
    device_id: DeviceId,
) -> Result<PrincipalId, ControlError> {
    let peer_json = connection
        .query_row(
            "SELECT peer_json FROM devices WHERE device_id = ?1 AND revoked = 0",
            [device_id.to_string()],
            |row| row.get::<_, String>(0),
        )
        .optional()
        .map_err(sqlite_error)?
        .ok_or_else(ControlError::authentication_failed)?;
    match serde_json::from_str(&peer_json).map_err(json_error)? {
        PeerIdentity::Surface { principal_id, .. } => Ok(principal_id),
        PeerIdentity::Node { .. } => Err(ControlError::role("a node holds no push token")),
    }
}

/// The device a surface connection authenticated as. A browser ticket
/// connection has none, so it cannot hold a token.
async fn connection_device(
    outgoing: &mpsc::Sender<ServerMessage>,
    request_id: u64,
    session: &SurfaceSession,
) -> Option<DeviceId> {
    let device_id = session.authority.device_id();
    if device_id.is_none() {
        send_error(
            outgoing,
            Some(request_id),
            ErrorCode::InvalidRole,
            "push tokens belong to an enrolled device; \
             this connection was opened with a browser ticket",
        )
        .await;
    }
    device_id
}

pub(crate) async fn handle(
    state: &CoordinatorState,
    outgoing: &mpsc::Sender<ServerMessage>,
    request_id: u64,
    session: &SurfaceSession,
    operation: PushOperation,
) {
    match operation {
        PushOperation::Set { provider, token } => {
            set_push_token(state, outgoing, request_id, session, &provider, token).await;
        }
        PushOperation::Clear => clear_push_token(state, outgoing, request_id, session).await,
    }
}

async fn set_push_token(
    state: &CoordinatorState,
    outgoing: &mpsc::Sender<ServerMessage>,
    request_id: u64,
    session: &SurfaceSession,
    provider: &str,
    token: PushToken,
) {
    let Some(device_id) = connection_device(outgoing, request_id, session).await else {
        return;
    };
    let provider = match validate(provider, &token) {
        Ok(provider) => provider,
        Err(error) => return send_control_error(outgoing, Some(request_id), &error).await,
    };
    match state.store.set_push_token(device_id, provider, token).await {
        Ok((principal_id, registration)) => {
            let (reason, moved_from) = match registration {
                Registration::Stored => ("stored", None),
                Registration::Unchanged => ("unchanged", None),
                Registration::Moved { from } => ("moved from another device", Some(from)),
            };
            control_log::event(
                "info",
                "push_token_set",
                &serde_json::json!({
                    "device_id": device_id,
                    "principal_id": principal_id,
                    "provider": provider.as_str(),
                    "reason": reason,
                    "moved_from_device_id": moved_from,
                }),
            );
            let _ = outgoing
                .send(ServerMessage::PushTokenSet { request_id })
                .await;
        }
        Err(error) => {
            if error.protocol_code() == ErrorCode::Conflict {
                control_log::event(
                    "warn",
                    "push_token_conflict",
                    &serde_json::json!({
                        "device_id": device_id,
                        "principal_id": session.principal_id,
                        "reason": "the token is registered to another principal",
                    }),
                );
            }
            send_control_error(outgoing, Some(request_id), &error).await;
        }
    }
}

async fn clear_push_token(
    state: &CoordinatorState,
    outgoing: &mpsc::Sender<ServerMessage>,
    request_id: u64,
    session: &SurfaceSession,
) {
    let Some(device_id) = connection_device(outgoing, request_id, session).await else {
        return;
    };
    match state.store.clear_push_token(device_id).await {
        Ok(removed) => {
            control_log::event(
                "info",
                "push_token_cleared",
                &serde_json::json!({
                    "device_id": device_id,
                    "principal_id": session.principal_id,
                    "reason": if removed { "cleared" } else { "the device held no token" },
                }),
            );
            let _ = outgoing
                .send(ServerMessage::PushTokenCleared { request_id })
                .await;
        }
        Err(error) => send_control_error(outgoing, Some(request_id), &error).await,
    }
}
