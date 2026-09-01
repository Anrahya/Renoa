use std::{sync::Arc, time::SystemTime};

use renoa_protocol::{PrincipalId, SurfaceRef};
use rusqlite::{Connection, OptionalExtension, TransactionBehavior, params};
use uuid::Uuid;
use webauthn_rs::prelude::{Passkey, PasskeyAuthentication, PasskeyRegistration};

use crate::{
    ControlError, PasskeyBootstrapToken,
    control_schema::open_connection,
    identity_store::timestamp_millis,
    store::{ControlStore, blocking, id_error, json_error, sqlite_error},
};

const MAX_ACTIVE_CEREMONIES: i64 = 64;

pub(crate) struct RegistrationBootstrap {
    pub(crate) principal_id: PrincipalId,
    pub(crate) passkeys: Vec<Passkey>,
}

pub(crate) struct RegistrationCeremony {
    pub(crate) principal_id: PrincipalId,
    pub(crate) surface: SurfaceRef,
    pub(crate) state: PasskeyRegistration,
}

pub(crate) struct AuthenticationCeremony {
    pub(crate) principal_id: PrincipalId,
    pub(crate) surface: SurfaceRef,
    pub(crate) state: PasskeyAuthentication,
}

impl ControlStore {
    pub(crate) async fn create_passkey_bootstrap(
        &self,
        principal_id: PrincipalId,
        expires_at: SystemTime,
    ) -> Result<PasskeyBootstrapToken, ControlError> {
        let token = PasskeyBootstrapToken::generate()?;
        let token_hash = token
            .digest()
            .ok_or_else(|| ControlError::store("generated an invalid passkey bootstrap token"))?;
        let expires_at_ms = timestamp_millis(expires_at)?;
        let path = Arc::clone(&self.path);
        blocking(move || {
            let connection = open_connection(&path)?;
            connection
                .execute(
                    "INSERT INTO passkey_bootstraps (token_hash, principal_id, expires_at_ms)
                     VALUES (?1, ?2, ?3)",
                    params![
                        token_hash.as_slice(),
                        principal_id.to_string(),
                        expires_at_ms,
                    ],
                )
                .map_err(sqlite_error)?;
            Ok(())
        })
        .await?;
        Ok(token)
    }

    pub(crate) async fn load_registration_bootstrap(
        &self,
        token: PasskeyBootstrapToken,
        now: SystemTime,
    ) -> Result<RegistrationBootstrap, ControlError> {
        let token_hash = token
            .digest()
            .ok_or_else(ControlError::authentication_failed)?;
        let now_ms = timestamp_millis(now)?;
        let path = Arc::clone(&self.path);
        blocking(move || {
            let connection = open_connection(&path)?;
            let principal_id = connection
                .query_row(
                    "SELECT principal_id FROM passkey_bootstraps
                     WHERE token_hash = ?1 AND expires_at_ms > ?2",
                    params![token_hash.as_slice(), now_ms],
                    |row| row.get::<_, String>(0),
                )
                .optional()
                .map_err(sqlite_error)?
                .ok_or_else(ControlError::authentication_failed)?;
            let principal_id = parse_principal(&principal_id)?;
            Ok(RegistrationBootstrap {
                principal_id,
                passkeys: load_passkeys(&connection, principal_id)?,
            })
        })
        .await
    }

    #[allow(
        clippy::too_many_arguments,
        reason = "these values are the complete one-time registration ceremony record"
    )]
    pub(crate) async fn save_registration_ceremony(
        &self,
        token: PasskeyBootstrapToken,
        principal_id: PrincipalId,
        surface: SurfaceRef,
        ceremony_id: Uuid,
        state: PasskeyRegistration,
        expires_at: SystemTime,
        now: SystemTime,
    ) -> Result<(), ControlError> {
        let token_hash = token
            .digest()
            .ok_or_else(ControlError::authentication_failed)?;
        let state_json = serde_json::to_string(&state).map_err(json_error)?;
        let expires_at_ms = timestamp_millis(expires_at)?;
        let now_ms = timestamp_millis(now)?;
        let path = Arc::clone(&self.path);
        blocking(move || {
            let mut connection = open_connection(&path)?;
            let transaction = connection
                .transaction_with_behavior(TransactionBehavior::Immediate)
                .map_err(sqlite_error)?;
            remove_expired_identity_records(&transaction, now_ms)?;
            ensure_ceremony_capacity(&transaction)?;
            let stored_principal = transaction
                .query_row(
                    "SELECT principal_id FROM passkey_bootstraps
                     WHERE token_hash = ?1 AND expires_at_ms > ?2",
                    params![token_hash.as_slice(), now_ms],
                    |row| row.get::<_, String>(0),
                )
                .optional()
                .map_err(sqlite_error)?
                .ok_or_else(ControlError::authentication_failed)?;
            if stored_principal != principal_id.to_string() {
                return Err(ControlError::authentication_failed());
            }
            if transaction
                .execute(
                    "DELETE FROM passkey_bootstraps WHERE token_hash = ?1",
                    [token_hash.as_slice()],
                )
                .map_err(sqlite_error)?
                != 1
            {
                return Err(ControlError::authentication_failed());
            }
            transaction
                .execute(
                    "INSERT INTO passkey_registration_ceremonies (
                        ceremony_id, principal_id, surface, state_json, expires_at_ms
                     ) VALUES (?1, ?2, ?3, ?4, ?5)",
                    params![
                        ceremony_id.to_string(),
                        principal_id.to_string(),
                        surface.as_str(),
                        state_json,
                        expires_at_ms,
                    ],
                )
                .map_err(sqlite_error)?;
            transaction.commit().map_err(sqlite_error)
        })
        .await
    }

    pub(crate) async fn claim_registration_ceremony(
        &self,
        ceremony_id: Uuid,
        now: SystemTime,
    ) -> Result<RegistrationCeremony, ControlError> {
        let now_ms = timestamp_millis(now)?;
        let path = Arc::clone(&self.path);
        blocking(move || {
            let mut connection = open_connection(&path)?;
            let transaction = connection
                .transaction_with_behavior(TransactionBehavior::Immediate)
                .map_err(sqlite_error)?;
            remove_expired_identity_records(&transaction, now_ms)?;
            let record = transaction
                .query_row(
                    "SELECT principal_id, surface, state_json
                     FROM passkey_registration_ceremonies
                     WHERE ceremony_id = ?1 AND expires_at_ms > ?2",
                    params![ceremony_id.to_string(), now_ms],
                    |row| {
                        Ok((
                            row.get::<_, String>(0)?,
                            row.get::<_, String>(1)?,
                            row.get::<_, String>(2)?,
                        ))
                    },
                )
                .optional()
                .map_err(sqlite_error)?
                .ok_or_else(ControlError::authentication_failed)?;
            transaction
                .execute(
                    "DELETE FROM passkey_registration_ceremonies WHERE ceremony_id = ?1",
                    [ceremony_id.to_string()],
                )
                .map_err(sqlite_error)?;
            transaction.commit().map_err(sqlite_error)?;
            Ok(RegistrationCeremony {
                principal_id: parse_principal(&record.0)?,
                surface: SurfaceRef::new(record.1),
                state: serde_json::from_str(&record.2).map_err(json_error)?,
            })
        })
        .await
    }

    pub(crate) async fn load_passkeys_for_authentication(
        &self,
        principal_id: PrincipalId,
    ) -> Result<Vec<Passkey>, ControlError> {
        let path = Arc::clone(&self.path);
        blocking(move || {
            let connection = open_connection(&path)?;
            let passkeys = load_passkeys(&connection, principal_id)?;
            if passkeys.is_empty() {
                return Err(ControlError::authentication_failed());
            }
            Ok(passkeys)
        })
        .await
    }

    pub(crate) async fn save_authentication_ceremony(
        &self,
        principal_id: PrincipalId,
        surface: SurfaceRef,
        ceremony_id: Uuid,
        state: PasskeyAuthentication,
        expires_at: SystemTime,
        now: SystemTime,
    ) -> Result<(), ControlError> {
        let state_json = serde_json::to_string(&state).map_err(json_error)?;
        let expires_at_ms = timestamp_millis(expires_at)?;
        let now_ms = timestamp_millis(now)?;
        let path = Arc::clone(&self.path);
        blocking(move || {
            let mut connection = open_connection(&path)?;
            let transaction = connection
                .transaction_with_behavior(TransactionBehavior::Immediate)
                .map_err(sqlite_error)?;
            remove_expired_identity_records(&transaction, now_ms)?;
            ensure_ceremony_capacity(&transaction)?;
            transaction
                .execute(
                    "INSERT INTO passkey_authentication_ceremonies (
                        ceremony_id, principal_id, surface, state_json, expires_at_ms
                     ) VALUES (?1, ?2, ?3, ?4, ?5)",
                    params![
                        ceremony_id.to_string(),
                        principal_id.to_string(),
                        surface.as_str(),
                        state_json,
                        expires_at_ms,
                    ],
                )
                .map_err(sqlite_error)?;
            transaction.commit().map_err(sqlite_error)
        })
        .await
    }

    pub(crate) async fn claim_authentication_ceremony(
        &self,
        ceremony_id: Uuid,
        now: SystemTime,
    ) -> Result<AuthenticationCeremony, ControlError> {
        let now_ms = timestamp_millis(now)?;
        let path = Arc::clone(&self.path);
        blocking(move || {
            let mut connection = open_connection(&path)?;
            let transaction = connection
                .transaction_with_behavior(TransactionBehavior::Immediate)
                .map_err(sqlite_error)?;
            remove_expired_identity_records(&transaction, now_ms)?;
            let record = transaction
                .query_row(
                    "SELECT principal_id, surface, state_json
                     FROM passkey_authentication_ceremonies
                     WHERE ceremony_id = ?1 AND expires_at_ms > ?2",
                    params![ceremony_id.to_string(), now_ms],
                    |row| {
                        Ok((
                            row.get::<_, String>(0)?,
                            row.get::<_, String>(1)?,
                            row.get::<_, String>(2)?,
                        ))
                    },
                )
                .optional()
                .map_err(sqlite_error)?
                .ok_or_else(ControlError::authentication_failed)?;
            transaction
                .execute(
                    "DELETE FROM passkey_authentication_ceremonies WHERE ceremony_id = ?1",
                    [ceremony_id.to_string()],
                )
                .map_err(sqlite_error)?;
            transaction.commit().map_err(sqlite_error)?;
            Ok(AuthenticationCeremony {
                principal_id: parse_principal(&record.0)?,
                surface: SurfaceRef::new(record.1),
                state: serde_json::from_str(&record.2).map_err(json_error)?,
            })
        })
        .await
    }
}

fn load_passkeys(
    connection: &Connection,
    principal_id: PrincipalId,
) -> Result<Vec<Passkey>, ControlError> {
    let mut statement = connection
        .prepare("SELECT passkey_json FROM passkeys WHERE principal_id = ?1 ORDER BY created_at_ms")
        .map_err(sqlite_error)?;
    let rows = statement
        .query_map([principal_id.to_string()], |row| row.get::<_, String>(0))
        .map_err(sqlite_error)?;
    let mut passkeys = Vec::new();
    for row in rows {
        passkeys.push(serde_json::from_str(&row.map_err(sqlite_error)?).map_err(json_error)?);
    }
    Ok(passkeys)
}

fn ensure_ceremony_capacity(connection: &Connection) -> Result<(), ControlError> {
    let active = connection
        .query_row(
            "SELECT
                (SELECT COUNT(*) FROM passkey_registration_ceremonies) +
                (SELECT COUNT(*) FROM passkey_authentication_ceremonies)",
            [],
            |row| row.get::<_, i64>(0),
        )
        .map_err(sqlite_error)?;
    if active >= MAX_ACTIVE_CEREMONIES {
        return Err(ControlError::capacity(
            "too many passkey ceremonies are active",
        ));
    }
    Ok(())
}

fn remove_expired_identity_records(
    connection: &Connection,
    now_ms: i64,
) -> Result<(), ControlError> {
    for table in [
        "passkey_bootstraps",
        "passkey_registration_ceremonies",
        "passkey_authentication_ceremonies",
        "browser_connection_tickets",
    ] {
        connection
            .execute(
                &format!("DELETE FROM {table} WHERE expires_at_ms <= ?1"),
                [now_ms],
            )
            .map_err(sqlite_error)?;
    }
    Ok(())
}

fn parse_principal(value: &str) -> Result<PrincipalId, ControlError> {
    Ok(PrincipalId::from_uuid(value.parse().map_err(id_error)?))
}
