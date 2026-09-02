use std::{sync::Arc, time::SystemTime};

use renoa_credential_relay_protocol::{
    CREDENTIAL_RELAY_VERSION, CredentialRelayForm, CredentialRelayId, CredentialRelayKind,
    CredentialRelayStatus,
};
use rusqlite::{OptionalExtension as _, TransactionBehavior, params};
use sha2::{Digest as _, Sha256};

use crate::{
    ControlError, DeviceId,
    identity_store::timestamp_millis,
    store::{ControlStore, blocking, sqlite_error},
};

pub(crate) const MAX_ACTIVE_RELAYS_PER_DEVICE: i64 = 8;
pub(crate) const MAX_ACTIVE_RELAYS_TOTAL: i64 = 128;

pub(crate) struct CredentialRelayReservation {
    pub(crate) relay_id: CredentialRelayId,
    pub(crate) expires_at_ms: i64,
}

impl ControlStore {
    pub(crate) async fn create_credential_relay(
        &self,
        device_id: DeviceId,
        relay_id: CredentialRelayId,
        credential_id: String,
        kind: CredentialRelayKind,
        capability_digest: String,
        expires_at: SystemTime,
    ) -> Result<CredentialRelayReservation, ControlError> {
        let now_ms = timestamp_millis(SystemTime::now())?;
        let expires_at_ms = timestamp_millis(expires_at)?;
        validate_reservation(&credential_id, &capability_digest, now_ms, expires_at_ms)?;
        let kind = kind_name(kind);
        let path = Arc::clone(&self.path);
        blocking(move || {
            let mut connection = crate::control_schema::open_connection(&path)?;
            let transaction = connection
                .transaction_with_behavior(TransactionBehavior::Immediate)
                .map_err(sqlite_error)?;
            delete_expired(&transaction, now_ms)?;
            require_active_device(&transaction, device_id)?;
            let stored = transaction
                .query_row(
                    "SELECT device_id, credential_id, kind, capability_digest, expires_at_ms
                     FROM credential_relays WHERE relay_id = ?1",
                    [relay_id.to_string()],
                    |row| {
                        Ok((
                            row.get::<_, String>(0)?,
                            row.get::<_, String>(1)?,
                            row.get::<_, String>(2)?,
                            row.get::<_, String>(3)?,
                            row.get::<_, i64>(4)?,
                        ))
                    },
                )
                .optional()
                .map_err(sqlite_error)?;
            if let Some(stored) = stored {
                if stored.0 != device_id.to_string()
                    || stored.1 != credential_id
                    || stored.2 != kind
                    || stored.3 != capability_digest
                {
                    return Err(ControlError::conflict(
                        "credential relay identity was already used with different content",
                    ));
                }
                transaction.commit().map_err(sqlite_error)?;
                return Ok(CredentialRelayReservation {
                    relay_id,
                    expires_at_ms: stored.4,
                });
            }
            if active_count(&transaction, Some(device_id), now_ms)? >= MAX_ACTIVE_RELAYS_PER_DEVICE
                || active_count(&transaction, None, now_ms)? >= MAX_ACTIVE_RELAYS_TOTAL
            {
                return Err(ControlError::capacity(
                    "too many credential relays are active",
                ));
            }
            transaction
                .execute(
                    "INSERT INTO credential_relays(
                         relay_id, device_id, credential_id, kind, capability_digest,
                         phase, created_at_ms, expires_at_ms
                     ) VALUES (?1, ?2, ?3, ?4, ?5, 'pending', ?6, ?7)",
                    params![
                        relay_id.to_string(),
                        device_id.to_string(),
                        credential_id,
                        kind,
                        capability_digest,
                        now_ms,
                        expires_at_ms,
                    ],
                )
                .map_err(sqlite_error)?;
            transaction.commit().map_err(sqlite_error)?;
            Ok(CredentialRelayReservation {
                relay_id,
                expires_at_ms,
            })
        })
        .await
    }

    pub(crate) async fn credential_relay_form(
        &self,
        relay_id: CredentialRelayId,
    ) -> Result<CredentialRelayForm, ControlError> {
        let now_ms = timestamp_millis(SystemTime::now())?;
        let path = Arc::clone(&self.path);
        blocking(move || {
            let connection = crate::control_schema::open_connection(&path)?;
            let row = connection
                .query_row(
                    "SELECT relay.credential_id, relay.kind, relay.expires_at_ms
                     FROM credential_relays AS relay
                     JOIN devices AS device ON device.device_id = relay.device_id
                     WHERE relay.relay_id = ?1 AND relay.expires_at_ms > ?2
                       AND relay.phase != 'acknowledged' AND device.revoked = 0",
                    params![relay_id.to_string(), now_ms],
                    |row| {
                        Ok((
                            row.get::<_, String>(0)?,
                            row.get::<_, String>(1)?,
                            row.get::<_, i64>(2)?,
                        ))
                    },
                )
                .optional()
                .map_err(sqlite_error)?
                .ok_or_else(|| ControlError::not_found("credential relay was not found"))?;
            Ok(CredentialRelayForm {
                version: CREDENTIAL_RELAY_VERSION,
                relay_id,
                credential_id: row.0,
                kind: parse_kind(&row.1)?,
                expires_at_ms: row.2,
            })
        })
        .await
    }

    pub(crate) async fn credential_relay_status(
        &self,
        device_id: DeviceId,
        relay_id: CredentialRelayId,
    ) -> Result<CredentialRelayStatus, ControlError> {
        let now_ms = timestamp_millis(SystemTime::now())?;
        let path = Arc::clone(&self.path);
        blocking(move || {
            let connection = crate::control_schema::open_connection(&path)?;
            let row = connection
                .query_row(
                    "SELECT relay.phase, relay.nonce, relay.ciphertext
                     FROM credential_relays AS relay
                     JOIN devices AS device ON device.device_id = relay.device_id
                     WHERE relay.relay_id = ?1 AND relay.device_id = ?2
                       AND relay.expires_at_ms > ?3 AND device.revoked = 0",
                    params![relay_id.to_string(), device_id.to_string(), now_ms],
                    |row| {
                        Ok((
                            row.get::<_, String>(0)?,
                            row.get::<_, Option<String>>(1)?,
                            row.get::<_, Option<String>>(2)?,
                        ))
                    },
                )
                .optional()
                .map_err(sqlite_error)?
                .ok_or_else(|| ControlError::not_found("credential relay was not found"))?;
            match row {
                (phase, None, None) if phase == "pending" => Ok(CredentialRelayStatus::Pending {
                    version: CREDENTIAL_RELAY_VERSION,
                }),
                (phase, Some(nonce), Some(ciphertext)) if phase == "submitted" => {
                    Ok(CredentialRelayStatus::Submitted {
                        version: CREDENTIAL_RELAY_VERSION,
                        nonce,
                        ciphertext,
                    })
                }
                (phase, None, None) if phase == "acknowledged" => {
                    Ok(CredentialRelayStatus::Acknowledged {
                        version: CREDENTIAL_RELAY_VERSION,
                    })
                }
                _ => Err(ControlError::store("stored credential relay is malformed")),
            }
        })
        .await
    }

    pub(crate) async fn submit_credential_relay(
        &self,
        relay_id: CredentialRelayId,
        capability: String,
        nonce: String,
        ciphertext: String,
    ) -> Result<(), ControlError> {
        validate_submission(&capability, &nonce, &ciphertext)?;
        let now_ms = timestamp_millis(SystemTime::now())?;
        let capability_digest = hex_sha256(capability.as_bytes());
        let result_digest = submission_digest(&nonce, &ciphertext);
        let path = Arc::clone(&self.path);
        blocking(move || {
            let mut connection = crate::control_schema::open_connection(&path)?;
            let transaction = connection
                .transaction_with_behavior(TransactionBehavior::Immediate)
                .map_err(sqlite_error)?;
            let stored = transaction
                .query_row(
                    "SELECT relay.phase, relay.capability_digest, relay.result_digest
                     FROM credential_relays AS relay
                     JOIN devices AS device ON device.device_id = relay.device_id
                     WHERE relay.relay_id = ?1 AND relay.expires_at_ms > ?2
                       AND device.revoked = 0",
                    params![relay_id.to_string(), now_ms],
                    |row| {
                        Ok((
                            row.get::<_, String>(0)?,
                            row.get::<_, String>(1)?,
                            row.get::<_, Option<Vec<u8>>>(2)?,
                        ))
                    },
                )
                .optional()
                .map_err(sqlite_error)?
                .ok_or_else(|| ControlError::not_found("credential relay was not found"))?;
            if !constant_time_equal(stored.1.as_bytes(), capability_digest.as_bytes()) {
                return Err(ControlError::authentication_failed());
            }
            match stored.0.as_str() {
                "pending" => {
                    transaction
                        .execute(
                            "UPDATE credential_relays
                             SET phase = 'submitted', nonce = ?1, ciphertext = ?2,
                                 result_digest = ?3
                             WHERE relay_id = ?4 AND phase = 'pending'",
                            params![
                                nonce,
                                ciphertext,
                                result_digest.as_slice(),
                                relay_id.to_string(),
                            ],
                        )
                        .map_err(sqlite_error)?;
                }
                "submitted" | "acknowledged"
                    if stored.2.as_deref() == Some(result_digest.as_slice()) => {}
                "submitted" | "acknowledged" => {
                    return Err(ControlError::conflict(
                        "credential relay was already submitted with different content",
                    ));
                }
                _ => return Err(ControlError::store("stored credential relay is malformed")),
            }
            transaction.commit().map_err(sqlite_error)
        })
        .await
    }

    pub(crate) async fn acknowledge_credential_relay(
        &self,
        device_id: DeviceId,
        relay_id: CredentialRelayId,
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
                    "SELECT phase FROM credential_relays
                     WHERE relay_id = ?1 AND device_id = ?2 AND expires_at_ms > ?3",
                    params![relay_id.to_string(), device_id.to_string(), now_ms],
                    |row| row.get::<_, String>(0),
                )
                .optional()
                .map_err(sqlite_error)?
                .ok_or_else(|| ControlError::not_found("credential relay was not found"))?;
            match phase.as_str() {
                "acknowledged" => {}
                "submitted" => {
                    transaction
                        .execute(
                            "UPDATE credential_relays
                             SET phase = 'acknowledged', nonce = NULL, ciphertext = NULL
                             WHERE relay_id = ?1",
                            [relay_id.to_string()],
                        )
                        .map_err(sqlite_error)?;
                }
                "pending" => {
                    return Err(ControlError::conflict("credential has not been submitted"));
                }
                _ => return Err(ControlError::store("stored credential relay is malformed")),
            }
            transaction.commit().map_err(sqlite_error)
        })
        .await
    }
}

fn validate_reservation(
    credential_id: &str,
    capability_digest: &str,
    now_ms: i64,
    expires_at_ms: i64,
) -> Result<(), ControlError> {
    let valid_id = !credential_id.is_empty()
        && credential_id.len() <= 128
        && credential_id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'));
    if !valid_id || !valid_hex(capability_digest, 64) || expires_at_ms <= now_ms {
        return Err(ControlError::invalid(
            "credential relay reservation is invalid",
        ));
    }
    Ok(())
}

fn validate_submission(
    capability: &str,
    nonce: &str,
    ciphertext: &str,
) -> Result<(), ControlError> {
    if !valid_hex(capability, 64)
        || !valid_hex(nonce, 24)
        || ciphertext.len() < 34
        || ciphertext.len() > 131_104
        || !ciphertext.len().is_multiple_of(2)
        || !valid_hex(ciphertext, ciphertext.len())
    {
        return Err(ControlError::invalid("credential submission is invalid"));
    }
    Ok(())
}

fn valid_hex(value: &str, length: usize) -> bool {
    value.len() == length
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'))
}

fn kind_name(kind: CredentialRelayKind) -> &'static str {
    match kind {
        CredentialRelayKind::ApiToken => "api_token",
        CredentialRelayKind::OAuthClient => "oauth_client",
    }
}

fn parse_kind(kind: &str) -> Result<CredentialRelayKind, ControlError> {
    match kind {
        "api_token" => Ok(CredentialRelayKind::ApiToken),
        "oauth_client" => Ok(CredentialRelayKind::OAuthClient),
        _ => Err(ControlError::store(
            "stored credential relay kind is malformed",
        )),
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
                "SELECT COUNT(*) FROM credential_relays
                 WHERE device_id = ?1 AND expires_at_ms > ?2
                   AND phase != 'acknowledged'",
                params![device_id.to_string(), now_ms],
                |row| row.get(0),
            )
            .map_err(sqlite_error),
        None => connection
            .query_row(
                "SELECT COUNT(*) FROM credential_relays
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
            "DELETE FROM credential_relays WHERE expires_at_ms <= ?1",
            [now_ms],
        )
        .map(|_| ())
        .map_err(sqlite_error)
}

fn submission_digest(nonce: &str, ciphertext: &str) -> [u8; 32] {
    let mut digest = Sha256::new();
    digest.update(b"renoa credential relay submission v1\0");
    digest.update(nonce.len().to_be_bytes());
    digest.update(nonce.as_bytes());
    digest.update(ciphertext.len().to_be_bytes());
    digest.update(ciphertext.as_bytes());
    digest.finalize().into()
}

fn hex_sha256(value: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let digest: [u8; 32] = Sha256::digest(value).into();
    let mut encoded = String::with_capacity(64);
    for byte in digest {
        encoded.push(char::from(HEX[usize::from(byte >> 4)]));
        encoded.push(char::from(HEX[usize::from(byte & 0x0f)]));
    }
    encoded
}

fn constant_time_equal(left: &[u8], right: &[u8]) -> bool {
    if left.len() != right.len() {
        return false;
    }
    left.iter()
        .zip(right)
        .fold(0_u8, |difference, (left, right)| {
            difference | (left ^ right)
        })
        == 0
}
