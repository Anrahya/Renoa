//! Devices that hold a registered key: the schema, sign-in, listing and the
//! owner's revocation. The device row itself stays in `identity_store`.

use std::{sync::Arc, time::SystemTime};

use renoa_protocol::{PrincipalId, SurfaceRef};
use rusqlite::{Connection, OptionalExtension, TransactionBehavior, params};
use uuid::Uuid;
use webauthn_rs::prelude::{AuthenticationResult, Passkey};

use crate::{
    ControlError, DeviceCredential, DeviceCredentials, DeviceId, DeviceSummary, PeerIdentity,
    browser_ticket_store::{advance_passkey, insert_passkey},
    control_schema::open_connection,
    device_key::{DevicePublicKey, DeviceRegistration},
    identity_store::timestamp_millis,
    store::{ControlStore, blocking, id_error, json_error, sqlite_error},
};

/// Active key-bound devices one principal may hold at once.
pub(crate) const MAX_DEVICES_PER_PRINCIPAL: i64 = 32;

pub(crate) fn create_schema(connection: &Connection) -> Result<(), ControlError> {
    connection
        .execute_batch(
            "CREATE TABLE IF NOT EXISTS device_keys (
                device_id TEXT PRIMARY KEY REFERENCES devices(device_id),
                principal_id TEXT NOT NULL,
                public_key BLOB NOT NULL UNIQUE CHECK (
                    length(public_key) = 65 AND hex(substr(public_key, 1, 1)) = '04'
                ),
                label TEXT NOT NULL CHECK (length(label) BETWEEN 1 AND 64),
                created_at_ms INTEGER NOT NULL CHECK (created_at_ms >= 0)
            );

            CREATE INDEX IF NOT EXISTS device_keys_principal
                ON device_keys(principal_id, created_at_ms);

            CREATE TABLE IF NOT EXISTS device_sign_ins (
                ceremony_id TEXT PRIMARY KEY,
                public_key BLOB NOT NULL CHECK (length(public_key) = 65),
                label TEXT NOT NULL CHECK (length(label) BETWEEN 1 AND 64),
                expires_at_ms INTEGER NOT NULL
            );",
        )
        .map_err(sqlite_error)
}

/// A device created or adopted by a sign-in.
#[derive(Debug)]
pub(crate) struct SignedInDevice {
    pub(crate) credentials: DeviceCredentials,
    pub(crate) adopted: bool,
}

enum Registration {
    Adopt(DeviceId),
    New,
}

/// Decides, without writing, what signing in `registration` would do. The
/// key is the identity the app owns before it first sends, so a repeat of the
/// same principal, surface and key adopts its device.
fn classify(
    connection: &Connection,
    principal_id: PrincipalId,
    surface: &SurfaceRef,
    registration: &DeviceRegistration,
) -> Result<Registration, ControlError> {
    let existing = connection
        .query_row(
            "SELECT d.device_id, d.revoked, d.peer_json
             FROM device_keys k JOIN devices d ON d.device_id = k.device_id
             WHERE k.public_key = ?1",
            [registration.key.as_bytes().as_slice()],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, bool>(1)?,
                    row.get::<_, String>(2)?,
                ))
            },
        )
        .optional()
        .map_err(sqlite_error)?;
    if let Some((device_id, revoked, peer_json)) = existing {
        let peer: PeerIdentity = serde_json::from_str(&peer_json).map_err(json_error)?;
        let expected = PeerIdentity::Surface {
            principal_id,
            surface: surface.clone(),
        };
        if peer != expected {
            return Err(ControlError::conflict(
                "this device key is registered to another sign-in",
            ));
        }
        if revoked {
            return Err(ControlError::conflict(
                "this device key was signed out; create a new key to sign in again",
            ));
        }
        return Ok(Registration::Adopt(DeviceId::from_uuid(
            device_id.parse().map_err(id_error)?,
        )));
    }
    let active = connection
        .query_row(
            "SELECT COUNT(*) FROM device_keys k JOIN devices d ON d.device_id = k.device_id
             WHERE k.principal_id = ?1 AND d.revoked = 0",
            [principal_id.to_string()],
            |row| row.get::<_, i64>(0),
        )
        .map_err(sqlite_error)?;
    if active >= MAX_DEVICES_PER_PRINCIPAL {
        return Err(ControlError::conflict(format!(
            "this account already has {MAX_DEVICES_PER_PRINCIPAL} signed-in devices; \
             sign one out first"
        )));
    }
    Ok(Registration::New)
}

/// Creates or adopts the device for a verified sign-in and issues its
/// credential. Runs inside the sign-in's transaction.
fn sign_in_device(
    connection: &Connection,
    principal_id: PrincipalId,
    surface: &SurfaceRef,
    registration: &DeviceRegistration,
    now_ms: i64,
) -> Result<SignedInDevice, ControlError> {
    let credential = DeviceCredential::generate()?;
    let credential_hash = credential
        .digest()
        .ok_or_else(|| ControlError::store("generated an invalid device credential"))?;
    let (device_id, adopted) = match classify(connection, principal_id, surface, registration)? {
        Registration::Adopt(device_id) => {
            connection
                .execute(
                    "UPDATE devices SET credential_hash = ?2 WHERE device_id = ?1",
                    params![device_id.to_string(), credential_hash.as_slice()],
                )
                .map_err(sqlite_error)?;
            connection
                .execute(
                    "UPDATE device_keys SET label = ?2 WHERE device_id = ?1",
                    params![device_id.to_string(), registration.label],
                )
                .map_err(sqlite_error)?;
            (device_id, true)
        }
        Registration::New => {
            let device_id = DeviceId::new();
            let peer = PeerIdentity::Surface {
                principal_id,
                surface: surface.clone(),
            };
            connection
                .execute(
                    "INSERT INTO devices (device_id, credential_hash, peer_json)
                     VALUES (?1, ?2, ?3)",
                    params![
                        device_id.to_string(),
                        credential_hash.as_slice(),
                        serde_json::to_string(&peer).map_err(json_error)?,
                    ],
                )
                .map_err(sqlite_error)?;
            connection
                .execute(
                    "INSERT INTO device_keys (
                        device_id, principal_id, public_key, label, created_at_ms
                     ) VALUES (?1, ?2, ?3, ?4, ?5)",
                    params![
                        device_id.to_string(),
                        principal_id.to_string(),
                        registration.key.as_bytes().as_slice(),
                        registration.label,
                        now_ms,
                    ],
                )
                .map_err(sqlite_error)?;
            (device_id, false)
        }
    };
    Ok(SignedInDevice {
        credentials: DeviceCredentials {
            device_id,
            credential,
        },
        adopted,
    })
}

/// Stores the key a sign-in ceremony was started for, beside the ceremony.
pub(crate) fn save_sign_in(
    connection: &Connection,
    ceremony_id: Uuid,
    registration: &DeviceRegistration,
    expires_at_ms: i64,
) -> Result<(), ControlError> {
    connection
        .execute(
            "INSERT INTO device_sign_ins (ceremony_id, public_key, label, expires_at_ms)
             VALUES (?1, ?2, ?3, ?4)",
            params![
                ceremony_id.to_string(),
                registration.key.as_bytes().as_slice(),
                registration.label,
                expires_at_ms,
            ],
        )
        .map_err(sqlite_error)?;
    Ok(())
}

/// Removes and returns the key a claimed ceremony was started for.
pub(crate) fn take_sign_in(
    connection: &Connection,
    ceremony_id: Uuid,
) -> Result<Option<DeviceRegistration>, ControlError> {
    let stored = connection
        .query_row(
            "SELECT public_key, label FROM device_sign_ins WHERE ceremony_id = ?1",
            [ceremony_id.to_string()],
            |row| Ok((row.get::<_, Vec<u8>>(0)?, row.get::<_, String>(1)?)),
        )
        .optional()
        .map_err(sqlite_error)?;
    connection
        .execute(
            "DELETE FROM device_sign_ins WHERE ceremony_id = ?1",
            [ceremony_id.to_string()],
        )
        .map_err(sqlite_error)?;
    stored
        .map(|(public_key, label)| {
            let key = DevicePublicKey::from_bytes(&public_key)
                .ok_or_else(|| ControlError::store("stored sign-in key is invalid"))?;
            Ok(DeviceRegistration { key, label })
        })
        .transpose()
}

impl ControlStore {
    /// Rejects, before a ceremony or bootstrap is spent, a sign-in that its
    /// transaction would refuse.
    pub(crate) async fn check_device_registration(
        &self,
        principal_id: PrincipalId,
        surface: SurfaceRef,
        registration: DeviceRegistration,
    ) -> Result<(), ControlError> {
        let path = Arc::clone(&self.path);
        blocking(move || {
            let connection = open_connection(&path)?;
            classify(&connection, principal_id, &surface, &registration).map(|_| ())
        })
        .await
    }

    pub(crate) async fn store_registration_and_device(
        &self,
        principal_id: PrincipalId,
        surface: SurfaceRef,
        passkey: Passkey,
        registration: DeviceRegistration,
        now: SystemTime,
    ) -> Result<SignedInDevice, ControlError> {
        let now_ms = timestamp_millis(now)?;
        let path = Arc::clone(&self.path);
        blocking(move || {
            let mut connection = open_connection(&path)?;
            let transaction = connection
                .transaction_with_behavior(TransactionBehavior::Immediate)
                .map_err(sqlite_error)?;
            insert_passkey(&transaction, principal_id, &passkey, now_ms)?;
            let device =
                sign_in_device(&transaction, principal_id, &surface, &registration, now_ms)?;
            transaction.commit().map_err(sqlite_error)?;
            Ok(device)
        })
        .await
    }

    pub(crate) async fn update_passkey_and_store_device(
        &self,
        principal_id: PrincipalId,
        surface: SurfaceRef,
        authentication: AuthenticationResult,
        registration: DeviceRegistration,
        now: SystemTime,
    ) -> Result<SignedInDevice, ControlError> {
        let now_ms = timestamp_millis(now)?;
        let path = Arc::clone(&self.path);
        blocking(move || {
            let mut connection = open_connection(&path)?;
            let transaction = connection
                .transaction_with_behavior(TransactionBehavior::Immediate)
                .map_err(sqlite_error)?;
            advance_passkey(&transaction, principal_id, &authentication)?;
            let device =
                sign_in_device(&transaction, principal_id, &surface, &registration, now_ms)?;
            transaction.commit().map_err(sqlite_error)?;
            Ok(device)
        })
        .await
    }

    /// The principal's active key-bound devices, oldest first. `current` marks
    /// the device the asking connection belongs to.
    pub(crate) async fn list_devices(
        &self,
        principal_id: PrincipalId,
        current: Option<DeviceId>,
    ) -> Result<Vec<DeviceSummary>, ControlError> {
        let path = Arc::clone(&self.path);
        blocking(move || {
            let connection = open_connection(&path)?;
            let mut statement = connection
                .prepare(
                    "SELECT d.device_id, d.peer_json, k.label, k.created_at_ms
                     FROM device_keys k JOIN devices d ON d.device_id = k.device_id
                     WHERE k.principal_id = ?1 AND d.revoked = 0
                     ORDER BY k.created_at_ms, d.device_id",
                )
                .map_err(sqlite_error)?;
            let rows = statement
                .query_map([principal_id.to_string()], |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, String>(2)?,
                        row.get::<_, i64>(3)?,
                    ))
                })
                .map_err(sqlite_error)?;
            let mut devices = Vec::new();
            for row in rows {
                let (device_id, peer_json, label, created_at_ms) = row.map_err(sqlite_error)?;
                let device_id = DeviceId::from_uuid(device_id.parse().map_err(id_error)?);
                let PeerIdentity::Surface { surface, .. } =
                    serde_json::from_str(&peer_json).map_err(json_error)?
                else {
                    return Err(ControlError::store("a key-bound device has no surface"));
                };
                devices.push(DeviceSummary {
                    device_id,
                    surface,
                    label,
                    created_at_ms: u64::try_from(created_at_ms)
                        .map_err(|_| ControlError::store("negative device creation time"))?,
                    current: current == Some(device_id),
                });
            }
            Ok(devices)
        })
        .await
    }

    /// Revokes one of the principal's key-bound devices. Returns whether this
    /// call revoked it; a device the principal already revoked succeeds again.
    pub(crate) async fn revoke_owned_device(
        &self,
        principal_id: PrincipalId,
        device_id: DeviceId,
    ) -> Result<bool, ControlError> {
        let path = Arc::clone(&self.path);
        blocking(move || {
            let mut connection = open_connection(&path)?;
            let transaction = connection
                .transaction_with_behavior(TransactionBehavior::Immediate)
                .map_err(sqlite_error)?;
            let revoked = transaction
                .query_row(
                    "SELECT d.revoked FROM device_keys k JOIN devices d ON d.device_id = k.device_id
                     WHERE k.device_id = ?1 AND k.principal_id = ?2",
                    params![device_id.to_string(), principal_id.to_string()],
                    |row| row.get::<_, bool>(0),
                )
                .optional()
                .map_err(sqlite_error)?
                .ok_or_else(|| ControlError::not_found(format!("device {device_id} was not found")))?;
            if !revoked {
                transaction
                    .execute(
                        "UPDATE devices SET revoked = 1 WHERE device_id = ?1",
                        [device_id.to_string()],
                    )
                    .map_err(sqlite_error)?;
            }
            transaction.commit().map_err(sqlite_error)?;
            Ok(!revoked)
        })
        .await
    }
}

#[cfg(test)]
mod tests;
