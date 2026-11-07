//! Where the coordinator keeps notice keys and sealed notices.

use std::{sync::Arc, time::SystemTime};

use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use renoa_protocol::PrincipalId;
use rusqlite::{Connection, OptionalExtension as _, TransactionBehavior, params};
use uuid::Uuid;

use super::{MAX_LISTED_NOTICES, MAX_SEALED_NOTICE_BYTES};
use crate::{
    ControlError, DeviceId, DeviceNoticeKey, HeldNotice, NodeId, NoticeKey, PeerIdentity,
    SealedNotice,
    control_schema::open_connection,
    identity_store::timestamp_millis,
    node_ownership::node_owner,
    store::{ControlStore, blocking, id_error, json_error, sqlite_error},
};

/// Keys and notices, keyed by device. `principal_id` is the device's
/// principal, copied when the key is set or the notice is put.
pub(crate) fn create_schema(connection: &Connection) -> Result<(), ControlError> {
    connection
        .execute_batch(&format!(
            "CREATE TABLE IF NOT EXISTS notice_keys (
                device_id TEXT PRIMARY KEY REFERENCES devices(device_id),
                principal_id TEXT NOT NULL,
                public_key TEXT NOT NULL CHECK (length(public_key) = 87)
            );

            CREATE TABLE IF NOT EXISTS setup_notices (
                notice_id TEXT PRIMARY KEY,
                device_id TEXT NOT NULL REFERENCES devices(device_id),
                principal_id TEXT NOT NULL,
                node_id TEXT NOT NULL,
                sealed TEXT NOT NULL CHECK (length(sealed) BETWEEN 1 AND {MAX_SEALED_NOTICE_BYTES}),
                expires_at_ms INTEGER NOT NULL
            );

            CREATE INDEX IF NOT EXISTS setup_notices_device
                ON setup_notices(device_id);"
        ))
        .map_err(sqlite_error)
}

/// Deletes the device's key and notices. Revocation calls it in the
/// transaction that revokes the device.
pub(crate) fn forget_device(
    connection: &Connection,
    device_id: DeviceId,
) -> Result<(), ControlError> {
    for table in ["notice_keys", "setup_notices"] {
        connection
            .execute(
                &format!("DELETE FROM {table} WHERE device_id = ?1"),
                [device_id.to_string()],
            )
            .map_err(sqlite_error)?;
    }
    Ok(())
}

/// The principal an active surface device was enrolled for.
fn surface_principal(
    connection: &Connection,
    device_id: DeviceId,
) -> Result<Option<PrincipalId>, ControlError> {
    let Some(peer_json) = connection
        .query_row(
            "SELECT peer_json FROM devices WHERE device_id = ?1 AND revoked = 0",
            [device_id.to_string()],
            |row| row.get::<_, String>(0),
        )
        .optional()
        .map_err(sqlite_error)?
    else {
        return Ok(None);
    };
    Ok(
        match serde_json::from_str(&peer_json).map_err(json_error)? {
            PeerIdentity::Surface { principal_id, .. } => Some(principal_id),
            PeerIdentity::Node { .. } => None,
        },
    )
}

pub(super) fn now_ms() -> Result<i64, ControlError> {
    timestamp_millis(SystemTime::now())
}

impl ControlStore {
    pub(super) async fn set_notice_key(
        &self,
        device_id: DeviceId,
        key: NoticeKey,
    ) -> Result<(), ControlError> {
        let path = Arc::clone(&self.path);
        blocking(move || {
            let mut connection = open_connection(&path)?;
            let transaction = connection
                .transaction_with_behavior(TransactionBehavior::Immediate)
                .map_err(sqlite_error)?;
            let principal_id = surface_principal(&transaction, device_id)?
                .ok_or_else(ControlError::authentication_failed)?;
            transaction
                .execute(
                    "INSERT INTO notice_keys (device_id, principal_id, public_key)
                     VALUES (?1, ?2, ?3)
                     ON CONFLICT (device_id) DO UPDATE SET public_key = excluded.public_key",
                    params![
                        device_id.to_string(),
                        principal_id.to_string(),
                        key.as_str()
                    ],
                )
                .map_err(sqlite_error)?;
            transaction.commit().map_err(sqlite_error)
        })
        .await
    }

    /// The device's unexpired notices, at most [`MAX_LISTED_NOTICES`] of them,
    /// soonest to expire first; expired ones are deleted first.
    pub(super) async fn device_notices(
        &self,
        device_id: DeviceId,
    ) -> Result<Vec<HeldNotice>, ControlError> {
        let path = Arc::clone(&self.path);
        blocking(move || {
            let now_ms = now_ms()?;
            let connection = open_connection(&path)?;
            connection
                .execute(
                    "DELETE FROM setup_notices WHERE expires_at_ms <= ?1",
                    [now_ms],
                )
                .map_err(sqlite_error)?;
            let mut statement = connection
                .prepare(
                    &format!(
                    "SELECT notice_id, sealed, expires_at_ms FROM setup_notices
                     WHERE device_id = ?1 ORDER BY expires_at_ms, notice_id LIMIT {MAX_LISTED_NOTICES}"
                ),
                )
                .map_err(sqlite_error)?;
            let rows = statement
                .query_map([device_id.to_string()], |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, i64>(2)?,
                    ))
                })
                .map_err(sqlite_error)?;
            let mut notices = Vec::new();
            for row in rows {
                let (notice_id, sealed, expires_at_ms) = row.map_err(sqlite_error)?;
                notices.push(HeldNotice {
                    notice_id: notice_id.parse().map_err(id_error)?,
                    sealed,
                    expires_at_ms,
                });
            }
            Ok(notices)
        })
        .await
    }

    pub(super) async fn acknowledge_notice(
        &self,
        device_id: DeviceId,
        notice_id: Uuid,
    ) -> Result<(), ControlError> {
        let path = Arc::clone(&self.path);
        blocking(move || {
            open_connection(&path)?
                .execute(
                    "DELETE FROM setup_notices WHERE notice_id = ?1 AND device_id = ?2",
                    params![notice_id.to_string(), device_id.to_string()],
                )
                .map_err(sqlite_error)?;
            Ok(())
        })
        .await
    }

    /// The keys of the node owner's active devices.
    pub(super) async fn owner_notice_keys(
        &self,
        node_id: NodeId,
    ) -> Result<Vec<DeviceNoticeKey>, ControlError> {
        let path = Arc::clone(&self.path);
        blocking(move || {
            let connection = open_connection(&path)?;
            let principal_id = owner(&connection, node_id)?;
            let mut statement = connection
                .prepare(
                    "SELECT k.device_id, k.public_key
                     FROM notice_keys k JOIN devices d ON d.device_id = k.device_id
                     WHERE k.principal_id = ?1 AND d.revoked = 0
                     ORDER BY k.device_id",
                )
                .map_err(sqlite_error)?;
            let rows = statement
                .query_map([principal_id.to_string()], |row| {
                    Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
                })
                .map_err(sqlite_error)?;
            let mut keys = Vec::new();
            for row in rows {
                let (device_id, public_key) = row.map_err(sqlite_error)?;
                keys.push(DeviceNoticeKey {
                    device_id: DeviceId::from_uuid(device_id.parse().map_err(id_error)?),
                    public_key: NoticeKey::new(public_key),
                });
            }
            Ok(keys)
        })
        .await
    }

    /// Stores `notice` for the node owner's device, unless its id is stored
    /// already, and marks a wake for the owner in the same transaction.
    /// Returns the owner when the notice is new.
    pub(super) async fn put_notice(
        &self,
        node_id: NodeId,
        notice: SealedNotice,
    ) -> Result<Option<PrincipalId>, ControlError> {
        let path = Arc::clone(&self.path);
        blocking(move || {
            let mut connection = open_connection(&path)?;
            let transaction = connection
                .transaction_with_behavior(TransactionBehavior::Immediate)
                .map_err(sqlite_error)?;
            let principal_id = owner(&transaction, node_id)?;
            let stored = transaction
                .query_row(
                    "SELECT device_id, node_id FROM setup_notices WHERE notice_id = ?1",
                    [notice.notice_id.to_string()],
                    |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
                )
                .optional()
                .map_err(sqlite_error)?;
            if let Some((device, node)) = stored {
                return if device == notice.device_id.to_string() && node == node_id.to_string() {
                    Ok(None)
                } else {
                    Err(ControlError::conflict(
                        "this notice id names another recipient; derive a new id",
                    ))
                };
            }
            let keyed: Option<String> = transaction
                .query_row(
                    "SELECT k.principal_id FROM notice_keys k JOIN devices d
                     ON d.device_id = k.device_id
                     WHERE k.device_id = ?1 AND d.revoked = 0",
                    [notice.device_id.to_string()],
                    |row| row.get(0),
                )
                .optional()
                .map_err(sqlite_error)?;
            if keyed.as_deref() != Some(principal_id.to_string().as_str()) {
                return Err(ControlError::role(
                    "the device is not an active device of this node's owner with a notice key; \
                     list the notice keys again",
                ));
            }
            transaction
                .execute(
                    "INSERT INTO setup_notices
                     (notice_id, device_id, principal_id, node_id, sealed, expires_at_ms)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                    params![
                        notice.notice_id.to_string(),
                        notice.device_id.to_string(),
                        principal_id.to_string(),
                        node_id.to_string(),
                        notice.sealed,
                        notice.expires_at_ms,
                    ],
                )
                .map_err(sqlite_error)?;
            crate::push_wake::record(&transaction, principal_id, false)?;
            transaction.commit().map_err(sqlite_error)?;
            Ok(Some(principal_id))
        })
        .await
    }

    pub(super) async fn withdraw_notices(
        &self,
        node_id: NodeId,
        notice_ids: Vec<Uuid>,
    ) -> Result<(), ControlError> {
        let path = Arc::clone(&self.path);
        blocking(move || {
            let mut connection = open_connection(&path)?;
            let transaction = connection.transaction().map_err(sqlite_error)?;
            for notice_id in notice_ids {
                transaction
                    .execute(
                        "DELETE FROM setup_notices WHERE notice_id = ?1 AND node_id = ?2",
                        params![notice_id.to_string(), node_id.to_string()],
                    )
                    .map_err(sqlite_error)?;
            }
            transaction.commit().map_err(sqlite_error)
        })
        .await
    }
}

fn owner(connection: &Connection, node_id: NodeId) -> Result<PrincipalId, ControlError> {
    node_owner(connection, node_id)?.ok_or_else(|| {
        ControlError::role(format!("node {node_id} has no owner, so it has no devices"))
    })
}

/// Checks a notice before anything is written. No error repeats the sealed
/// text.
pub(super) fn validate(notice: &SealedNotice, now_ms: i64) -> Result<(), ControlError> {
    let length = notice.sealed.len();
    if length == 0
        || length > MAX_SEALED_NOTICE_BYTES
        || URL_SAFE_NO_PAD.decode(&notice.sealed).is_err()
    {
        return Err(ControlError::invalid(format!(
            "sealed must be 1 to {MAX_SEALED_NOTICE_BYTES} bytes of base64url without padding; \
             received {length} bytes"
        )));
    }
    if notice.expires_at_ms <= now_ms {
        return Err(ControlError::invalid(
            "the notice has expired; seal the link again only while it works",
        ));
    }
    Ok(())
}
