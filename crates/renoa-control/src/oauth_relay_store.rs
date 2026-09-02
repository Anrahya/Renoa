use std::{sync::Arc, time::SystemTime};

use renoa_oauth_relay_protocol::{OAUTH_RELAY_VERSION, OAuthRelayId, OAuthRelayStatus};
use rusqlite::{OptionalExtension as _, TransactionBehavior, params};
use sha2::{Digest as _, Sha256};

use crate::{
    ControlError, DeviceId,
    identity_store::timestamp_millis,
    store::{ControlStore, blocking, sqlite_error},
};

pub(crate) const MAX_ACTIVE_RELAYS_PER_DEVICE: i64 = 8;
pub(crate) const MAX_ACTIVE_RELAYS_TOTAL: i64 = 128;

pub(crate) struct OAuthRelayReservation {
    pub(crate) relay_id: OAuthRelayId,
    pub(crate) expires_at_ms: i64,
}

pub(crate) enum OAuthCallbackResult<'a> {
    Authorized {
        authorization_code: &'a str,
        issuer: Option<&'a str>,
    },
    Rejected {
        error: &'a str,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum OAuthCallbackAdmission {
    Authorized,
    Rejected,
}

impl ControlStore {
    pub(crate) async fn create_oauth_relay(
        &self,
        device_id: DeviceId,
        relay_id: OAuthRelayId,
        state_digest: String,
        expires_at: SystemTime,
    ) -> Result<OAuthRelayReservation, ControlError> {
        let now_ms = timestamp_millis(SystemTime::now())?;
        let expires_at_ms = timestamp_millis(expires_at)?;
        if expires_at_ms <= now_ms {
            return Err(ControlError::invalid(
                "OAuth relay expiry must be in the future",
            ));
        }
        let path = Arc::clone(&self.path);
        blocking(move || {
            let mut connection = crate::control_schema::open_connection(&path)?;
            let transaction = connection
                .transaction_with_behavior(TransactionBehavior::Immediate)
                .map_err(sqlite_error)?;
            delete_expired(&transaction, now_ms)?;
            require_active_device(&transaction, device_id)?;

            let by_id = transaction
                .query_row(
                    "SELECT device_id, state_digest, expires_at_ms
                     FROM oauth_callback_relays WHERE relay_id = ?1",
                    [relay_id.to_string()],
                    |row| {
                        Ok((
                            row.get::<_, String>(0)?,
                            row.get::<_, String>(1)?,
                            row.get::<_, i64>(2)?,
                        ))
                    },
                )
                .optional()
                .map_err(sqlite_error)?;
            if let Some((stored_device, stored_state, stored_expiry)) = by_id {
                if stored_device != device_id.to_string() || stored_state != state_digest {
                    return Err(ControlError::conflict(
                        "OAuth relay identity was already used with different content",
                    ));
                }
                transaction.commit().map_err(sqlite_error)?;
                return Ok(OAuthRelayReservation {
                    relay_id,
                    expires_at_ms: stored_expiry,
                });
            }
            let state_exists = transaction
                .query_row(
                    "SELECT EXISTS(
                        SELECT 1 FROM oauth_callback_relays WHERE state_digest = ?1
                     )",
                    [&state_digest],
                    |row| row.get::<_, bool>(0),
                )
                .map_err(sqlite_error)?;
            if state_exists {
                return Err(ControlError::conflict(
                    "OAuth state was already bound to another relay",
                ));
            }
            let per_device = active_count(&transaction, Some(device_id), now_ms)?;
            let total = active_count(&transaction, None, now_ms)?;
            if per_device >= MAX_ACTIVE_RELAYS_PER_DEVICE || total >= MAX_ACTIVE_RELAYS_TOTAL {
                return Err(ControlError::capacity(
                    "too many OAuth callback relays are active",
                ));
            }
            transaction
                .execute(
                    "INSERT INTO oauth_callback_relays(
                         relay_id, device_id, state_digest, phase, created_at_ms, expires_at_ms
                     ) VALUES (?1, ?2, ?3, 'pending', ?4, ?5)",
                    params![
                        relay_id.to_string(),
                        device_id.to_string(),
                        state_digest,
                        now_ms,
                        expires_at_ms,
                    ],
                )
                .map_err(sqlite_error)?;
            transaction.commit().map_err(sqlite_error)?;
            Ok(OAuthRelayReservation {
                relay_id,
                expires_at_ms,
            })
        })
        .await
    }

    pub(crate) async fn oauth_relay_status(
        &self,
        device_id: DeviceId,
        relay_id: OAuthRelayId,
    ) -> Result<OAuthRelayStatus, ControlError> {
        let now_ms = timestamp_millis(SystemTime::now())?;
        let path = Arc::clone(&self.path);
        blocking(move || {
            let connection = crate::control_schema::open_connection(&path)?;
            let row = connection
                .query_row(
                    "SELECT relay.phase, relay.authorization_code, relay.issuer, relay.oauth_error
                     FROM oauth_callback_relays AS relay
                     JOIN devices AS device ON device.device_id = relay.device_id
                     WHERE relay.relay_id = ?1 AND relay.device_id = ?2
                       AND relay.expires_at_ms > ?3 AND device.revoked = 0",
                    params![relay_id.to_string(), device_id.to_string(), now_ms],
                    |row| {
                        Ok((
                            row.get::<_, String>(0)?,
                            row.get::<_, Option<String>>(1)?,
                            row.get::<_, Option<String>>(2)?,
                            row.get::<_, Option<String>>(3)?,
                        ))
                    },
                )
                .optional()
                .map_err(sqlite_error)?
                .ok_or_else(|| ControlError::not_found("OAuth relay was not found"))?;
            match row {
                (phase, None, None, None) if phase == "pending" => Ok(OAuthRelayStatus::Pending {
                    version: OAUTH_RELAY_VERSION,
                }),
                (phase, Some(authorization_code), issuer, None) if phase == "authorized" => {
                    Ok(OAuthRelayStatus::Authorized {
                        version: OAUTH_RELAY_VERSION,
                        authorization_code,
                        issuer,
                    })
                }
                (phase, None, None, Some(error)) if phase == "rejected" => {
                    Ok(OAuthRelayStatus::Rejected {
                        version: OAUTH_RELAY_VERSION,
                        error,
                    })
                }
                (phase, None, None, None) if phase == "acknowledged" => {
                    Ok(OAuthRelayStatus::Acknowledged {
                        version: OAUTH_RELAY_VERSION,
                    })
                }
                _ => Err(ControlError::store("stored OAuth relay is malformed")),
            }
        })
        .await
    }

    pub(crate) async fn acknowledge_oauth_relay(
        &self,
        device_id: DeviceId,
        relay_id: OAuthRelayId,
    ) -> Result<(), ControlError> {
        let now_ms = timestamp_millis(SystemTime::now())?;
        let path = Arc::clone(&self.path);
        blocking(move || {
            let mut connection = crate::control_schema::open_connection(&path)?;
            let transaction = connection
                .transaction_with_behavior(TransactionBehavior::Immediate)
                .map_err(sqlite_error)?;
            require_active_device(&transaction, device_id)?;
            let phase = transaction
                .query_row(
                    "SELECT phase FROM oauth_callback_relays
                     WHERE relay_id = ?1 AND device_id = ?2 AND expires_at_ms > ?3",
                    params![relay_id.to_string(), device_id.to_string(), now_ms],
                    |row| row.get::<_, String>(0),
                )
                .optional()
                .map_err(sqlite_error)?
                .ok_or_else(|| ControlError::not_found("OAuth relay was not found"))?;
            match phase.as_str() {
                "acknowledged" => {}
                "authorized" | "rejected" => {
                    transaction
                        .execute(
                            "UPDATE oauth_callback_relays
                             SET phase = 'acknowledged', authorization_code = NULL,
                                 issuer = NULL, oauth_error = NULL
                             WHERE relay_id = ?1",
                            [relay_id.to_string()],
                        )
                        .map_err(sqlite_error)?;
                }
                "pending" => {
                    return Err(ControlError::conflict("OAuth callback has not arrived"));
                }
                _ => return Err(ControlError::store("stored OAuth relay is malformed")),
            }
            transaction.commit().map_err(sqlite_error)
        })
        .await
    }

    pub(crate) async fn record_oauth_callback(
        &self,
        state_digest: String,
        result: OAuthCallbackResult<'_>,
    ) -> Result<OAuthCallbackAdmission, ControlError> {
        let now_ms = timestamp_millis(SystemTime::now())?;
        let (kind, authorization_code, issuer, oauth_error, result_digest, admission) = match result
        {
            OAuthCallbackResult::Authorized {
                authorization_code,
                issuer,
            } => (
                "authorized",
                Some(authorization_code.to_owned()),
                issuer.map(str::to_owned),
                None,
                callback_digest("authorized", authorization_code, issuer),
                OAuthCallbackAdmission::Authorized,
            ),
            OAuthCallbackResult::Rejected { error } => (
                "rejected",
                None,
                None,
                Some(error.to_owned()),
                callback_digest("rejected", error, None),
                OAuthCallbackAdmission::Rejected,
            ),
        };
        let path = Arc::clone(&self.path);
        blocking(move || {
            let mut connection = crate::control_schema::open_connection(&path)?;
            let transaction = connection
                .transaction_with_behavior(TransactionBehavior::Immediate)
                .map_err(sqlite_error)?;
            delete_expired(&transaction, now_ms)?;
            let stored = transaction
                .query_row(
                    "SELECT relay.phase, relay.result_kind, relay.result_digest
                     FROM oauth_callback_relays AS relay
                     JOIN devices AS device ON device.device_id = relay.device_id
                     WHERE relay.state_digest = ?1 AND relay.expires_at_ms > ?2
                       AND device.revoked = 0",
                    params![state_digest, now_ms],
                    |row| {
                        Ok((
                            row.get::<_, String>(0)?,
                            row.get::<_, Option<String>>(1)?,
                            row.get::<_, Option<Vec<u8>>>(2)?,
                        ))
                    },
                )
                .optional()
                .map_err(sqlite_error)?
                .ok_or_else(|| ControlError::not_found("OAuth relay was not found"))?;
            if stored.0 == "pending" {
                transaction
                    .execute(
                        "UPDATE oauth_callback_relays
                         SET phase = ?1, result_kind = ?1, authorization_code = ?2,
                             issuer = ?3, oauth_error = ?4, result_digest = ?5
                         WHERE state_digest = ?6 AND phase = 'pending'",
                        params![
                            kind,
                            authorization_code,
                            issuer,
                            oauth_error,
                            result_digest.as_slice(),
                            state_digest,
                        ],
                    )
                    .map_err(sqlite_error)?;
            } else if stored.1.as_deref() != Some(kind)
                || stored.2.as_deref() != Some(result_digest.as_slice())
            {
                return Err(ControlError::conflict(
                    "OAuth callback was already completed with different content",
                ));
            }
            transaction.commit().map_err(sqlite_error)?;
            Ok(admission)
        })
        .await
    }
}

fn require_active_device(
    connection: &rusqlite::Connection,
    device_id: DeviceId,
) -> Result<(), ControlError> {
    let active = connection
        .query_row(
            "SELECT EXISTS(
                SELECT 1 FROM devices WHERE device_id = ?1 AND revoked = 0
             )",
            [device_id.to_string()],
            |row| row.get::<_, bool>(0),
        )
        .map_err(sqlite_error)?;
    if active {
        Ok(())
    } else {
        Err(ControlError::authentication_failed())
    }
}

fn active_count(
    connection: &rusqlite::Connection,
    device_id: Option<DeviceId>,
    now_ms: i64,
) -> Result<i64, ControlError> {
    match device_id {
        Some(device_id) => connection
            .query_row(
                "SELECT COUNT(*) FROM oauth_callback_relays
                 WHERE device_id = ?1 AND expires_at_ms > ?2
                   AND phase != 'acknowledged'",
                params![device_id.to_string(), now_ms],
                |row| row.get(0),
            )
            .map_err(sqlite_error),
        None => connection
            .query_row(
                "SELECT COUNT(*) FROM oauth_callback_relays
                 WHERE expires_at_ms > ?1 AND phase != 'acknowledged'",
                [now_ms],
                |row| row.get(0),
            )
            .map_err(sqlite_error),
    }
}

fn delete_expired(connection: &rusqlite::Connection, now_ms: i64) -> Result<(), ControlError> {
    connection
        .execute(
            "DELETE FROM oauth_callback_relays WHERE expires_at_ms <= ?1",
            [now_ms],
        )
        .map(|_| ())
        .map_err(sqlite_error)
}

fn callback_digest(kind: &str, value: &str, issuer: Option<&str>) -> [u8; 32] {
    let mut digest = Sha256::new();
    digest.update(b"renoa oauth callback result v1\0");
    update_part(&mut digest, kind.as_bytes());
    update_part(&mut digest, value.as_bytes());
    update_part(&mut digest, issuer.unwrap_or_default().as_bytes());
    digest.finalize().into()
}

fn update_part(digest: &mut Sha256, bytes: &[u8]) {
    digest.update(bytes.len().to_be_bytes());
    digest.update(bytes);
}
