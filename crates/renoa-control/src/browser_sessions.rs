//! Persistent browser identity, independent of any transport connection or Host runtime.

use std::{
    path::{Path, PathBuf},
    sync::Arc,
    time::{Duration, SystemTime},
};

use axum::http::{HeaderMap, HeaderValue, header};
use renoa_protocol::{PrincipalId, SurfaceRef};
use rusqlite::{Connection, OptionalExtension, TransactionBehavior, params};

use crate::{
    ConnectionTicket, ControlError,
    browser_identity::TicketGrant,
    browser_ticket_store::insert_ticket,
    identity::BrowserSessionToken,
    identity_store::timestamp_millis,
    store::{blocking, id_error, sqlite_error},
};

// A remembered browser renews after half its 180-day lifetime. Neither IP address
// nor a process-local secret participates, so network changes/restarts preserve login.
pub(crate) const SESSION_LIFETIME: Duration = Duration::from_hours(180 * 24);
pub(crate) const COOKIE_NAME: &str = "__Host-renoa_session";
pub(crate) const CLEAR_COOKIE: &str =
    "__Host-renoa_session=; Path=/; Secure; HttpOnly; SameSite=Strict; Max-Age=0";

#[derive(Clone)]
pub struct BrowserSessions {
    path: Arc<PathBuf>,
}

/// An authenticated identity, not authorization to manage any particular Host.
pub struct BrowserSession {
    principal: PrincipalId,
    token: BrowserSessionToken,
    expires_at_ms: i64,
}

impl BrowserSession {
    #[must_use]
    pub const fn principal_id(&self) -> PrincipalId {
        self.principal
    }

    /// Refreshes the browser cookie to the persisted expiry, without exposing it in JSON.
    /// # Errors
    /// Returns an invalid clock or cookie encoding error.
    pub fn cookie(&self, now: SystemTime) -> Result<HeaderValue, ControlError> {
        session_cookie(&self.token, self.expires_at_ms, now)
    }
}

impl BrowserSessions {
    /// Opens existing identity storage. Does not initialize a coordinator or a Host.
    /// # Errors
    /// Returns missing/incompatible identity storage and filesystem failures.
    pub fn open(path: impl AsRef<Path>) -> Result<Self, ControlError> {
        let path = std::fs::canonicalize(path).map_err(|error| {
            ControlError::store(format!("identity database unavailable: {error}"))
        })?;
        let db = Connection::open_with_flags(&path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
            .map_err(sqlite_error)?;
        db.prepare("SELECT token_hash, credential_id, expires_at_ms FROM browser_sessions")
            .map_err(sqlite_error)?;
        Ok(Self {
            path: Arc::new(path),
        })
    }

    /// Validates and renews a remembered browser against durable identity storage.
    /// Unknown/expired cookies return `None`; storage failures remain errors.
    /// Cancelling may leave a harmless expiry renewal completing in the background.
    /// # Errors
    /// Returns storage or clock failures; these must not be treated as logout.
    pub async fn authenticate(
        &self,
        headers: &HeaderMap,
        now: SystemTime,
    ) -> Result<Option<BrowserSession>, ControlError> {
        let Some(token) = cookie_token(headers) else {
            return Ok(None);
        };
        let Some(hash) = token.digest() else {
            return Ok(None);
        };
        let now_ms = timestamp_millis(now)?;
        let expiry = session_expiry(now)?;
        let path = Arc::clone(&self.path);
        blocking(move || {
            let mut db = existing_connection(&path)?;
            let tx = db
                .transaction_with_behavior(TransactionBehavior::Immediate)
                .map_err(sqlite_error)?;
            let found: Option<(String, i64)> = tx
                .query_row(
                    "SELECT p.principal_id, s.expires_at_ms FROM browser_sessions s
                 JOIN passkeys p ON p.credential_id=s.credential_id
                 WHERE s.token_hash=?1 AND s.expires_at_ms>?2",
                    params![hash.as_slice(), now_ms],
                    |r| Ok((r.get(0)?, r.get(1)?)),
                )
                .optional()
                .map_err(sqlite_error)?;
            let Some((principal, mut expires_at_ms)) = found else {
                return Ok(None);
            };
            if expires_at_ms - now_ms < (expiry - now_ms) / 2 {
                tx.execute(
                    "UPDATE browser_sessions SET expires_at_ms=?2 WHERE token_hash=?1",
                    params![hash.as_slice(), expiry],
                )
                .map_err(sqlite_error)?;
                expires_at_ms = expiry;
            }
            tx.commit().map_err(sqlite_error)?;
            Ok(Some(BrowserSession {
                principal: PrincipalId::from_uuid(principal.parse().map_err(id_error)?),
                token,
                expires_at_ms,
            }))
        })
        .await
    }

    /// Revokes this browser only. Repetition and absent cookies are harmless.
    /// # Errors
    /// Returns a storage failure; a lost response can safely be retried.
    pub async fn logout(&self, headers: &HeaderMap) -> Result<(), ControlError> {
        let Some(hash) = cookie_token(headers).and_then(|token| token.digest()) else {
            return Ok(());
        };
        let path = Arc::clone(&self.path);
        blocking(move || {
            existing_connection(&path)?
                .execute(
                    "DELETE FROM browser_sessions WHERE token_hash=?1",
                    [hash.as_slice()],
                )
                .map_err(sqlite_error)?;
            Ok(())
        })
        .await
    }

    /// Revokes all remembered browser logins for an owner through trusted local administration.
    /// # Errors
    /// Returns a storage failure.
    pub async fn revoke(&self, principal: PrincipalId) -> Result<(), ControlError> {
        let path = Arc::clone(&self.path);
        blocking(move || {
            existing_connection(&path)?
                .execute(
                    "DELETE FROM browser_sessions WHERE credential_id IN
                (SELECT credential_id FROM passkeys WHERE principal_id=?1)",
                    [principal.to_string()],
                )
                .map_err(sqlite_error)?;
            Ok(())
        })
        .await
    }

    pub(crate) async fn connection_ticket(
        &self,
        session: &BrowserSession,
        surface: SurfaceRef,
        now: SystemTime,
    ) -> Result<Option<ConnectionTicket>, ControlError> {
        let hash = session
            .token
            .digest()
            .ok_or_else(ControlError::authentication_failed)?;
        let ticket = ConnectionTicket::generate()?;
        let ticket_hash = ticket
            .digest()
            .ok_or_else(ControlError::authentication_failed)?;
        let now_ms = timestamp_millis(now)?;
        let expires = now_ms
            .checked_add(60_000)
            .ok_or_else(|| ControlError::store("ticket expiry overflow"))?;
        let path = Arc::clone(&self.path);
        blocking(move || {
            let mut db = existing_connection(&path)?;
            let tx = db.transaction_with_behavior(TransactionBehavior::Immediate).map_err(sqlite_error)?;
            // Recheck inside issuance transaction so logout cannot race a new admission.
            let principal: Option<String> = tx.query_row("SELECT p.principal_id FROM browser_sessions s
                JOIN passkeys p ON p.credential_id=s.credential_id WHERE s.token_hash=?1 AND s.expires_at_ms>?2",
                params![hash.as_slice(), now_ms], |r| r.get(0)).optional().map_err(sqlite_error)?;
            let Some(principal) = principal else { return Ok(None) };
            tx.execute("DELETE FROM browser_connection_tickets WHERE expires_at_ms<=?1", [now_ms]).map_err(sqlite_error)?;
            insert_ticket(&tx, &ticket_hash, PrincipalId::from_uuid(principal.parse().map_err(id_error)?), &surface, expires)?;
            tx.commit().map_err(sqlite_error)?;
            Ok(Some(ticket))
        }).await
    }
}

fn existing_connection(path: &Path) -> Result<Connection, ControlError> {
    let db = Connection::open_with_flags(path, rusqlite::OpenFlags::SQLITE_OPEN_READ_WRITE)
        .map_err(sqlite_error)?;
    db.execute_batch("PRAGMA foreign_keys=ON; PRAGMA synchronous=FULL; PRAGMA busy_timeout=5000;")
        .map_err(sqlite_error)?;
    Ok(db)
}

pub(crate) fn insert_session(
    db: &Connection,
    credential: &[u8],
    grant: &TicketGrant,
) -> Result<(), ControlError> {
    let hash = grant
        .session
        .digest()
        .ok_or_else(ControlError::authentication_failed)?;
    let expiry = timestamp_millis(grant.session_expires_at)?;
    db.execute(
        "DELETE FROM browser_sessions WHERE expires_at_ms<=?1",
        [timestamp_millis(SystemTime::now())?],
    )
    .map_err(sqlite_error)?;
    db.execute(
        "INSERT INTO browser_sessions(token_hash,credential_id,expires_at_ms) VALUES(?1,?2,?3)",
        params![hash.as_slice(), credential, expiry],
    )
    .map_err(sqlite_error)?;
    Ok(())
}

pub(crate) fn session_cookie(
    token: &BrowserSessionToken,
    expiry_ms: i64,
    now: SystemTime,
) -> Result<HeaderValue, ControlError> {
    let max_age = (expiry_ms - timestamp_millis(now)?).max(0) / 1000;
    HeaderValue::from_str(&format!(
        "{COOKIE_NAME}={}; Path=/; Secure; HttpOnly; SameSite=Strict; Max-Age={max_age}",
        token.expose()
    ))
    .map_err(|_| ControlError::store("browser session cookie encoding failed"))
}

fn cookie_token(headers: &HeaderMap) -> Option<BrowserSessionToken> {
    let mut found = None;
    for header in headers.get_all(header::COOKIE) {
        for item in header.to_str().ok()?.split(';') {
            if let Some((name, value)) = item.trim().split_once('=')
                && name == COOKIE_NAME
            {
                if found.is_some() {
                    return None;
                }
                found = Some(BrowserSessionToken::from_encoded(value)?);
            }
        }
    }
    found
}

fn session_expiry(now: SystemTime) -> Result<i64, ControlError> {
    timestamp_millis(
        now.checked_add(SESSION_LIFETIME)
            .ok_or_else(|| ControlError::store("session expiry overflow"))?,
    )
}
