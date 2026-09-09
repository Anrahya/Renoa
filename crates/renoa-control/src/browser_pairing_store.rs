use std::{path::PathBuf, sync::Arc, time::SystemTime};

use renoa_protocol::PrincipalId;
use rusqlite::{OptionalExtension, TransactionBehavior, params};

use crate::{
    BrowserPairingToken, ControlError,
    browser_sessions::{SESSION_LIFETIME, existing_connection},
    identity::BrowserSessionToken,
    identity_store::timestamp_millis,
    store::{blocking, id_error, sqlite_error},
};

pub(crate) async fn create(
    path: Arc<PathBuf>,
    principal: PrincipalId,
    expires_at: SystemTime,
) -> Result<BrowserPairingToken, ControlError> {
    let now = timestamp_millis(SystemTime::now())?;
    let expiry = timestamp_millis(expires_at)?;
    if expiry <= now {
        return Err(ControlError::invalid(
            "browser pairing expiry must be in the future",
        ));
    }
    let token = BrowserPairingToken::generate()?;
    let hash = token
        .digest()
        .ok_or_else(ControlError::authentication_failed)?;
    blocking(move || {
        let mut db = existing_connection(&path)?;
        let tx = db
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(sqlite_error)?;
        tx.execute(
            "DELETE FROM browser_pairings WHERE expires_at_ms<=?1",
            [now],
        )
        .map_err(sqlite_error)?;
        tx.execute(
            "INSERT INTO browser_pairings(token_hash,principal_id,expires_at_ms) VALUES(?1,?2,?3)",
            params![hash.as_slice(), principal.to_string(), expiry],
        )
        .map_err(sqlite_error)?;
        tx.commit().map_err(sqlite_error)?;
        Ok(token)
    })
    .await
}

pub(crate) async fn redeem(
    path: Arc<PathBuf>,
    token: BrowserPairingToken,
    session: BrowserSessionToken,
    now: SystemTime,
) -> Result<(PrincipalId, i64), ControlError> {
    let hash = token
        .digest()
        .ok_or_else(ControlError::authentication_failed)?;
    let session_hash = session
        .digest()
        .ok_or_else(ControlError::authentication_failed)?;
    let now_ms = timestamp_millis(now)?;
    let expiry = timestamp_millis(
        now.checked_add(SESSION_LIFETIME)
            .ok_or_else(|| ControlError::store("browser session expiry overflow"))?,
    )?;
    blocking(move || {
        let mut db = existing_connection(&path)?;
        let tx = db.transaction_with_behavior(TransactionBehavior::Immediate).map_err(sqlite_error)?;
        let grant: Option<(String, Option<Vec<u8>>)> = tx.query_row(
            "SELECT principal_id,claimed_session_hash FROM browser_pairings WHERE token_hash=?1 AND expires_at_ms>?2",
            params![hash.as_slice(), now_ms], |r| Ok((r.get(0)?,r.get(1)?)),
        ).optional().map_err(sqlite_error)?;
        let (principal, claimed) = grant.ok_or_else(ControlError::authentication_failed)?;
        let principal_id = PrincipalId::from_uuid(principal.parse().map_err(id_error)?);
        let expires_at_ms = if let Some(claimed) = claimed {
            if claimed != session_hash {
                return Err(ControlError::authentication_failed());
            }
            // Retry only the original admission. Logout/revocation must not resurrect it.
            tx.query_row(
                "SELECT expires_at_ms FROM browser_sessions WHERE token_hash=?1 AND principal_id=?2 AND expires_at_ms>?3",
                params![session_hash.as_slice(), principal, now_ms], |r| r.get(0),
            ).optional().map_err(sqlite_error)?.ok_or_else(ControlError::authentication_failed)?
        } else {
            tx.execute("DELETE FROM browser_sessions WHERE expires_at_ms<=?1", [now_ms]).map_err(sqlite_error)?;
            tx.execute(
                "INSERT INTO browser_sessions(token_hash,principal_id,expires_at_ms) VALUES(?1,?2,?3)",
                params![session_hash.as_slice(), principal, expiry],
            ).map_err(sqlite_error)?;
            tx.execute(
                "UPDATE browser_pairings SET claimed_session_hash=?2 WHERE token_hash=?1",
                params![hash.as_slice(), session_hash.as_slice()],
            ).map_err(sqlite_error)?;
            expiry
        };
        tx.commit().map_err(sqlite_error)?;
        Ok((principal_id, expires_at_ms))
    }).await
}
