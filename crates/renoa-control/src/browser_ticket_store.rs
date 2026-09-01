use std::{sync::Arc, time::SystemTime};

use renoa_protocol::{PrincipalId, SurfaceRef};
use rusqlite::{Connection, OptionalExtension, TransactionBehavior, params};
use webauthn_rs::prelude::{AuthenticationResult, Passkey};

use crate::{
    ConnectionTicket, ControlError, PeerIdentity,
    control_schema::open_connection,
    identity_store::timestamp_millis,
    store::{ControlStore, blocking, id_error, json_error, sqlite_error},
};

impl ControlStore {
    pub(crate) async fn store_registration_and_ticket(
        &self,
        principal_id: PrincipalId,
        surface: SurfaceRef,
        passkey: Passkey,
        ticket: ConnectionTicket,
        ticket_expires_at: SystemTime,
        now: SystemTime,
    ) -> Result<(), ControlError> {
        let credential_id = passkey.cred_id().as_ref().to_vec();
        let passkey_json = serde_json::to_string(&passkey).map_err(json_error)?;
        let ticket_hash = ticket
            .digest()
            .ok_or_else(|| ControlError::store("generated an invalid connection ticket"))?;
        let ticket_expires_at_ms = timestamp_millis(ticket_expires_at)?;
        let created_at_ms = timestamp_millis(now)?;
        let path = Arc::clone(&self.path);
        blocking(move || {
            let mut connection = open_connection(&path)?;
            let transaction = connection
                .transaction_with_behavior(TransactionBehavior::Immediate)
                .map_err(sqlite_error)?;
            transaction
                .execute(
                    "INSERT INTO passkeys (
                        credential_id, principal_id, passkey_json, authentication_counter,
                        created_at_ms
                     ) VALUES (?1, ?2, ?3, 0, ?4)",
                    params![
                        credential_id,
                        principal_id.to_string(),
                        passkey_json,
                        created_at_ms
                    ],
                )
                .map_err(registration_insert_error)?;
            insert_ticket(
                &transaction,
                &ticket_hash,
                principal_id,
                &surface,
                ticket_expires_at_ms,
            )?;
            transaction.commit().map_err(sqlite_error)
        })
        .await
    }

    #[allow(
        clippy::too_many_arguments,
        reason = "these values form the complete authenticated ticket-issuance transaction"
    )]
    pub(crate) async fn update_passkey_and_store_ticket(
        &self,
        principal_id: PrincipalId,
        surface: SurfaceRef,
        authentication: AuthenticationResult,
        ticket: ConnectionTicket,
        ticket_expires_at: SystemTime,
        now: SystemTime,
    ) -> Result<(), ControlError> {
        let credential_id = authentication.cred_id().as_ref().to_vec();
        let observed_counter = i64::from(authentication.counter());
        let ticket_hash = ticket
            .digest()
            .ok_or_else(|| ControlError::store("generated an invalid connection ticket"))?;
        let ticket_expires_at_ms = timestamp_millis(ticket_expires_at)?;
        let now_ms = timestamp_millis(now)?;
        let path = Arc::clone(&self.path);
        blocking(move || {
            let mut connection = open_connection(&path)?;
            let transaction = connection
                .transaction_with_behavior(TransactionBehavior::Immediate)
                .map_err(sqlite_error)?;
            remove_expired_tickets(&transaction, now_ms)?;
            let (passkey_json, stored_counter) = transaction
                .query_row(
                    "SELECT passkey_json, authentication_counter FROM passkeys
                     WHERE credential_id = ?1 AND principal_id = ?2",
                    params![credential_id, principal_id.to_string()],
                    |row| Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?)),
                )
                .optional()
                .map_err(sqlite_error)?
                .ok_or_else(ControlError::authentication_failed)?;
            if stored_counter > 0 && observed_counter <= stored_counter {
                return Err(ControlError::authentication_failed());
            }
            let mut passkey: Passkey = serde_json::from_str(&passkey_json).map_err(json_error)?;
            passkey
                .update_credential(&authentication)
                .ok_or_else(ControlError::authentication_failed)?;
            let updated_json = serde_json::to_string(&passkey).map_err(json_error)?;
            transaction
                .execute(
                    "UPDATE passkeys
                     SET passkey_json = ?3, authentication_counter = ?4
                     WHERE credential_id = ?1 AND principal_id = ?2",
                    params![
                        credential_id,
                        principal_id.to_string(),
                        updated_json,
                        observed_counter.max(stored_counter),
                    ],
                )
                .map_err(sqlite_error)?;
            insert_ticket(
                &transaction,
                &ticket_hash,
                principal_id,
                &surface,
                ticket_expires_at_ms,
            )?;
            transaction.commit().map_err(sqlite_error)
        })
        .await
    }

    pub(crate) async fn claim_connection_ticket(
        &self,
        ticket: ConnectionTicket,
        now: SystemTime,
    ) -> Result<PeerIdentity, ControlError> {
        let ticket_hash = ticket
            .digest()
            .ok_or_else(ControlError::authentication_failed)?;
        let now_ms = timestamp_millis(now)?;
        let path = Arc::clone(&self.path);
        blocking(move || {
            let mut connection = open_connection(&path)?;
            let transaction = connection
                .transaction_with_behavior(TransactionBehavior::Immediate)
                .map_err(sqlite_error)?;
            remove_expired_tickets(&transaction, now_ms)?;
            let peer = transaction
                .query_row(
                    "SELECT principal_id, surface FROM browser_connection_tickets
                     WHERE ticket_hash = ?1 AND expires_at_ms > ?2",
                    params![ticket_hash.as_slice(), now_ms],
                    |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
                )
                .optional()
                .map_err(sqlite_error)?
                .ok_or_else(ControlError::authentication_failed)?;
            if transaction
                .execute(
                    "DELETE FROM browser_connection_tickets WHERE ticket_hash = ?1",
                    [ticket_hash.as_slice()],
                )
                .map_err(sqlite_error)?
                != 1
            {
                return Err(ControlError::authentication_failed());
            }
            transaction.commit().map_err(sqlite_error)?;
            Ok(PeerIdentity::Surface {
                principal_id: PrincipalId::from_uuid(peer.0.parse().map_err(id_error)?),
                surface: SurfaceRef::new(peer.1),
            })
        })
        .await
    }
}

fn insert_ticket(
    connection: &Connection,
    ticket_hash: &[u8; 32],
    principal_id: PrincipalId,
    surface: &SurfaceRef,
    expires_at_ms: i64,
) -> Result<(), ControlError> {
    connection
        .execute(
            "INSERT INTO browser_connection_tickets (
                ticket_hash, principal_id, surface, expires_at_ms
             ) VALUES (?1, ?2, ?3, ?4)",
            params![
                ticket_hash.as_slice(),
                principal_id.to_string(),
                surface.as_str(),
                expires_at_ms,
            ],
        )
        .map_err(sqlite_error)?;
    Ok(())
}

fn remove_expired_tickets(connection: &Connection, now_ms: i64) -> Result<(), ControlError> {
    connection
        .execute(
            "DELETE FROM browser_connection_tickets WHERE expires_at_ms <= ?1",
            [now_ms],
        )
        .map_err(sqlite_error)?;
    Ok(())
}

#[allow(
    clippy::needless_pass_by_value,
    reason = "the owned signature is required by Result::map_err"
)]
fn registration_insert_error(error: rusqlite::Error) -> ControlError {
    if matches!(
        error.sqlite_error_code(),
        Some(rusqlite::ErrorCode::ConstraintViolation)
    ) {
        ControlError::authentication_failed()
    } else {
        sqlite_error(error)
    }
}
