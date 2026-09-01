use std::{sync::Arc, time::SystemTime};

use rusqlite::{OptionalExtension, TransactionBehavior, params};

use crate::{
    ControlError, DeviceCredential, DeviceCredentials, DeviceId, EnrollmentToken, PeerIdentity,
    store::{ControlStore, blocking, json_error, sqlite_error},
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct AuthenticatedDevice {
    pub(crate) device_id: DeviceId,
    pub(crate) peer: PeerIdentity,
}

impl ControlStore {
    pub(crate) async fn create_enrollment(
        &self,
        peer: PeerIdentity,
        expires_at: SystemTime,
    ) -> Result<EnrollmentToken, ControlError> {
        let token = EnrollmentToken::generate()?;
        let token_hash = token
            .digest()
            .ok_or_else(|| ControlError::store("generated an invalid enrollment token"))?;
        let expires_at_ms = timestamp_millis(expires_at)?;
        let peer_json = serde_json::to_string(&peer).map_err(json_error)?;
        let path = Arc::clone(&self.path);
        blocking(move || {
            let connection = crate::control_schema::open_connection(&path)?;
            connection
                .execute(
                    "INSERT INTO enrollments (token_hash, peer_json, expires_at_ms)
                     VALUES (?1, ?2, ?3)",
                    params![token_hash.as_slice(), peer_json, expires_at_ms],
                )
                .map_err(sqlite_error)?;
            Ok(())
        })
        .await?;
        Ok(token)
    }

    pub(crate) async fn claim_enrollment(
        &self,
        token: EnrollmentToken,
    ) -> Result<DeviceCredentials, ControlError> {
        let token_hash = token
            .digest()
            .ok_or_else(ControlError::authentication_failed)?;
        let credential = DeviceCredential::generate()?;
        let credential_hash = credential
            .digest()
            .ok_or_else(|| ControlError::store("generated an invalid device credential"))?;
        let device_id = DeviceId::new();
        let now_ms = timestamp_millis(SystemTime::now())?;
        let path = Arc::clone(&self.path);
        blocking(move || {
            let mut connection = crate::control_schema::open_connection(&path)?;
            let transaction = connection
                .transaction_with_behavior(TransactionBehavior::Immediate)
                .map_err(sqlite_error)?;
            let peer_json = transaction
                .query_row(
                    "SELECT peer_json FROM enrollments
                     WHERE token_hash = ?1 AND expires_at_ms > ?2",
                    params![token_hash.as_slice(), now_ms],
                    |row| row.get::<_, String>(0),
                )
                .optional()
                .map_err(sqlite_error)?
                .ok_or_else(ControlError::authentication_failed)?;
            let deleted = transaction
                .execute(
                    "DELETE FROM enrollments WHERE token_hash = ?1",
                    [token_hash.as_slice()],
                )
                .map_err(sqlite_error)?;
            if deleted != 1 {
                return Err(ControlError::authentication_failed());
            }
            transaction
                .execute(
                    "INSERT INTO devices (device_id, credential_hash, peer_json)
                     VALUES (?1, ?2, ?3)",
                    params![device_id.to_string(), credential_hash.as_slice(), peer_json,],
                )
                .map_err(sqlite_error)?;
            transaction.commit().map_err(sqlite_error)?;
            Ok(())
        })
        .await?;
        Ok(DeviceCredentials {
            device_id,
            credential,
        })
    }

    pub(crate) async fn authenticate_device(
        &self,
        credentials: DeviceCredentials,
    ) -> Result<AuthenticatedDevice, ControlError> {
        let credential_hash = credentials
            .credential
            .digest()
            .ok_or_else(ControlError::authentication_failed)?;
        let device_id = credentials.device_id;
        let path = Arc::clone(&self.path);
        blocking(move || {
            let connection = crate::control_schema::open_connection(&path)?;
            let peer_json = connection
                .query_row(
                    "SELECT peer_json FROM devices
                     WHERE device_id = ?1 AND credential_hash = ?2 AND revoked = 0",
                    params![device_id.to_string(), credential_hash.as_slice()],
                    |row| row.get::<_, String>(0),
                )
                .optional()
                .map_err(sqlite_error)?
                .ok_or_else(ControlError::authentication_failed)?;
            Ok(AuthenticatedDevice {
                device_id,
                peer: serde_json::from_str(&peer_json).map_err(json_error)?,
            })
        })
        .await
    }

    pub(crate) async fn revoke_device(&self, device_id: DeviceId) -> Result<(), ControlError> {
        let path = Arc::clone(&self.path);
        blocking(move || {
            let connection = crate::control_schema::open_connection(&path)?;
            let changed = connection
                .execute(
                    "UPDATE devices SET revoked = 1 WHERE device_id = ?1",
                    [device_id.to_string()],
                )
                .map_err(sqlite_error)?;
            if changed == 0 {
                return Err(ControlError::not_found(format!(
                    "device {device_id} was not found"
                )));
            }
            Ok(())
        })
        .await
    }

    pub(crate) async fn device_is_active(&self, device_id: DeviceId) -> Result<bool, ControlError> {
        let path = Arc::clone(&self.path);
        blocking(move || {
            let connection = crate::control_schema::open_connection(&path)?;
            connection
                .query_row(
                    "SELECT EXISTS(
                        SELECT 1 FROM devices WHERE device_id = ?1 AND revoked = 0
                     )",
                    [device_id.to_string()],
                    |row| row.get::<_, bool>(0),
                )
                .map_err(sqlite_error)
        })
        .await
    }
}

pub(crate) fn timestamp_millis(time: SystemTime) -> Result<i64, ControlError> {
    let millis = time
        .duration_since(SystemTime::UNIX_EPOCH)
        .map_err(|_| ControlError::invalid("time is before the Unix epoch"))?
        .as_millis();
    i64::try_from(millis).map_err(|_| ControlError::invalid("time exceeds SQLite i64 range"))
}
