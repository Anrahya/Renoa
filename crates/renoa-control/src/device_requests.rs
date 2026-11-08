//! Signing a computer in from a phone. A computer with no passkey asks, with
//! a key it made, for a sign-in and shows the code it is given. The owner
//! types that code into an app that proved its own key (`approve_device`),
//! which names the principal. The computer then collects its credential.
//!
//! The coordinator stores the code's digest only. The computer chose the
//! request's id and signs it with its key, so asking again converges on one
//! request and only the key's holder may collect what was approved. An
//! approval lasts only while the app that gave it is signed in.

use std::{
    sync::Arc,
    time::{Duration, SystemTime},
};

use renoa_protocol::{PrincipalId, SurfaceRef};
use renoa_rcp_wire::device_request_message;
use rusqlite::{Connection, OptionalExtension as _, TransactionBehavior, params};
use sha2::{Digest as _, Sha256};
use uuid::Uuid;

use crate::{
    ControlError, DeviceCode, DeviceId,
    browser_identity_store::MAX_ACTIVE_CEREMONIES,
    control_schema::open_connection,
    device_key::{DevicePublicKey, DeviceRegistration, DeviceSignature},
    device_store::{SignedInDevice, check_sign_in, sign_in_device},
    identity_store::timestamp_millis,
    store::{ControlStore, blocking, id_error, sqlite_error},
};

/// How long a computer waits for its owner: the time a passkey ceremony
/// gives a person, who here unlocks a phone and types a code instead.
const REQUEST_LIFETIME: Duration = Duration::from_mins(5);
/// Requests waiting at once. Like a passkey ceremony, a request is started
/// without authentication, so it shares the ceremonies' bound.
const MAX_ACTIVE_REQUESTS: i64 = MAX_ACTIVE_CEREMONIES;
/// Crockford's base32: no I, L, O or U, so a code read aloud or typed from
/// a screen is not misread.
const CODE_ALPHABET: &[u8; 32] = b"0123456789ABCDEFGHJKMNPQRSTVWXYZ";
/// Eight symbols carry 40 bits: with every request live, one guess finds one
/// with a chance of 2^-34.
const CODE_LENGTH: usize = 8;
const CODE_DOMAIN: &[u8] = b"renoa device request code v1\0";
/// Codes a principal may get wrong within one request lifetime. A person
/// reading a code off a screen mistypes it a few times, not more; with five
/// guesses a window, a principal finds another's code with a chance of at
/// most 5 * 2^-34 every five minutes.
const MAX_MISSES: i64 = 5;

pub(crate) fn create_schema(connection: &Connection) -> Result<(), ControlError> {
    connection
        .execute_batch(
            "CREATE TABLE IF NOT EXISTS device_requests (
                request_id TEXT PRIMARY KEY,
                surface TEXT NOT NULL,
                public_key BLOB NOT NULL CHECK (
                    length(public_key) = 65 AND hex(substr(public_key, 1, 1)) = '04'
                ),
                label TEXT NOT NULL CHECK (length(label) BETWEEN 1 AND 64),
                code_hash BLOB NOT NULL UNIQUE CHECK (length(code_hash) = 32),
                principal_id TEXT,
                approver_device_id TEXT,
                expires_at_ms INTEGER NOT NULL,
                CHECK ((principal_id IS NULL) = (approver_device_id IS NULL))
            );

            CREATE TABLE IF NOT EXISTS device_request_misses (
                principal_id TEXT NOT NULL,
                missed_at_ms INTEGER NOT NULL
            );

            CREATE INDEX IF NOT EXISTS device_request_misses_principal
                ON device_request_misses(principal_id, missed_at_ms);",
        )
        .map_err(sqlite_error)
}

/// A computer's request whose key signed it: the only way to make one, so a
/// stored request always proved its key.
pub(crate) struct NewRequest {
    request_id: Uuid,
    surface: SurfaceRef,
    registration: DeviceRegistration,
}

impl NewRequest {
    pub(crate) fn proven(
        request_id: Uuid,
        surface: SurfaceRef,
        registration: DeviceRegistration,
        proof: &str,
    ) -> Result<Self, ControlError> {
        if !proves(&registration.key, request_id, proof) {
            return Err(ControlError::invalid(
                "deviceProof is not the key's signature over \
                 \"renoa device request v1\\n<requestId>\"",
            ));
        }
        Ok(Self {
            request_id,
            surface,
            registration,
        })
    }
}

/// The code a computer shows its owner, and when it stops working.
pub(crate) struct OpenedRequest {
    pub(crate) code: String,
    pub(crate) expires_at_ms: i64,
}

/// What a computer collecting its sign-in finds.
pub(crate) enum Collected {
    /// No app has approved it yet.
    Waiting { expires_at_ms: i64 },
    /// Signed in, as a new device or the one this key already holds.
    SignedIn(PrincipalId, SignedInDevice),
}

struct StoredRequest {
    surface: SurfaceRef,
    registration: DeviceRegistration,
    principal_id: Option<PrincipalId>,
    expires_at_ms: i64,
}

/// A fresh code, as the computer shows it.
fn new_code() -> Result<String, ControlError> {
    let mut bytes = [0_u8; CODE_LENGTH];
    getrandom::fill(&mut bytes).map_err(|error| {
        ControlError::store(format!("secure random generation failed: {error}"))
    })?;
    Ok(bytes
        .iter()
        .map(|byte| char::from(CODE_ALPHABET[usize::from(byte & 0x1f)]))
        .collect())
}

/// The digest a code is stored and found by, for a code as a person may type
/// it: in either case, with spaces or dashes, and with O, I or L for the
/// digits they look like. `None` when it cannot be a code.
fn code_digest(typed: &str) -> Option<[u8; 32]> {
    let mut code = String::with_capacity(CODE_LENGTH);
    for symbol in typed.chars().filter(|symbol| !matches!(symbol, ' ' | '-')) {
        let symbol = match symbol.to_ascii_uppercase() {
            'O' => '0',
            'I' | 'L' => '1',
            other => other,
        };
        if !symbol.is_ascii() || !CODE_ALPHABET.contains(&(symbol as u8)) {
            return None;
        }
        code.push(symbol);
    }
    (code.len() == CODE_LENGTH).then(|| {
        let mut digest = Sha256::new();
        digest.update(CODE_DOMAIN);
        digest.update(code.as_bytes());
        digest.finalize().into()
    })
}

/// Whether `proof` is the request key's signature over the request.
fn proves(key: &DevicePublicKey, request_id: Uuid, proof: &str) -> bool {
    DeviceSignature::parse(proof)
        .is_some_and(|signature| key.verify(&device_request_message(request_id), &signature))
}

fn remove_expired(connection: &Connection, now_ms: i64) -> Result<(), ControlError> {
    let window_ms = i64::try_from(REQUEST_LIFETIME.as_millis())
        .map_err(|_| ControlError::store("request lifetime is out of range"))?;
    connection
        .execute(
            "DELETE FROM device_requests WHERE expires_at_ms <= ?1",
            [now_ms],
        )
        .map_err(sqlite_error)?;
    connection
        .execute(
            "DELETE FROM device_request_misses WHERE missed_at_ms <= ?1",
            [now_ms - window_ms],
        )
        .map(|_| ())
        .map_err(sqlite_error)
}

/// Withdraws the approvals `device_id` gave, so a computer it approved and
/// that has not collected yet waits again. Runs in the revocation's
/// transaction.
pub(crate) fn forget_approver(
    connection: &Connection,
    device_id: DeviceId,
) -> Result<(), ControlError> {
    connection
        .execute(
            "UPDATE device_requests SET principal_id = NULL, approver_device_id = NULL
             WHERE approver_device_id = ?1",
            [device_id.to_string()],
        )
        .map(|_| ())
        .map_err(sqlite_error)
}

fn stored(
    connection: &Connection,
    request_id: Uuid,
) -> Result<Option<StoredRequest>, ControlError> {
    let row = connection
        .query_row(
            "SELECT surface, public_key, label, principal_id, expires_at_ms
             FROM device_requests WHERE request_id = ?1",
            [request_id.to_string()],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, Vec<u8>>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, Option<String>>(3)?,
                    row.get::<_, i64>(4)?,
                ))
            },
        )
        .optional()
        .map_err(sqlite_error)?;
    row.map(
        |(surface, public_key, label, principal_id, expires_at_ms)| {
            let key = DevicePublicKey::from_bytes(&public_key)
                .ok_or_else(|| ControlError::store("stored device request key is invalid"))?;
            Ok(StoredRequest {
                surface: SurfaceRef::new(surface),
                registration: DeviceRegistration { key, label },
                principal_id: principal_id
                    .map(|value| value.parse().map(PrincipalId::from_uuid).map_err(id_error))
                    .transpose()?,
                expires_at_ms,
            })
        },
    )
    .transpose()
}

impl ControlStore {
    /// Stores a computer's request and returns the code it shows. Asking
    /// again for a request no app has approved replaces its code and keeps
    /// its expiry, so a lost answer converges on one request.
    pub(crate) async fn open_device_request(
        &self,
        request: NewRequest,
        now: SystemTime,
    ) -> Result<OpenedRequest, ControlError> {
        let code = new_code()?;
        let code_hash =
            code_digest(&code).ok_or_else(|| ControlError::store("generated an invalid code"))?;
        let now_ms = timestamp_millis(now)?;
        let new_expiry_ms = timestamp_millis(now + REQUEST_LIFETIME)?;
        let path = Arc::clone(&self.path);
        blocking(move || {
            let mut connection = open_connection(&path)?;
            let transaction = connection
                .transaction_with_behavior(TransactionBehavior::Immediate)
                .map_err(sqlite_error)?;
            remove_expired(&transaction, now_ms)?;
            let expires_at_ms = if let Some(existing) = stored(&transaction, request.request_id)? {
                if existing.surface != request.surface
                    || existing.registration != request.registration
                {
                    return Err(ControlError::conflict(
                        "this request id is taken by another request; choose a new one",
                    ));
                }
                if existing.principal_id.is_some() {
                    return Err(ControlError::conflict(
                        "this request was approved; collect its credential",
                    ));
                }
                transaction
                    .execute(
                        "UPDATE device_requests SET code_hash = ?2 WHERE request_id = ?1",
                        params![request.request_id.to_string(), code_hash.as_slice()],
                    )
                    .map_err(sqlite_error)?;
                existing.expires_at_ms
            } else {
                let active = transaction
                    .query_row("SELECT COUNT(*) FROM device_requests", [], |row| {
                        row.get::<_, i64>(0)
                    })
                    .map_err(sqlite_error)?;
                if active >= MAX_ACTIVE_REQUESTS {
                    return Err(ControlError::capacity(
                        "too many computers are waiting to sign in",
                    ));
                }
                transaction
                    .execute(
                        "INSERT INTO device_requests (
                            request_id, surface, public_key, label, code_hash, expires_at_ms
                         ) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                        params![
                            request.request_id.to_string(),
                            request.surface.as_str(),
                            request.registration.key.as_bytes().as_slice(),
                            request.registration.label,
                            code_hash.as_slice(),
                            new_expiry_ms,
                        ],
                    )
                    .map_err(sqlite_error)?;
                new_expiry_ms
            };
            transaction.commit().map_err(sqlite_error)?;
            Ok(OpenedRequest {
                code,
                expires_at_ms,
            })
        })
        .await
    }

    /// Approves, for `principal_id` by its app `approver`, the waiting
    /// request that shows `code`, once the sign-in it leads to would be
    /// accepted. Approving it again for the same principal succeeds again. A
    /// code that finds no request is a miss; a principal with too many
    /// recent misses is refused before the code is looked up.
    pub(crate) async fn approve_device_request(
        &self,
        principal_id: PrincipalId,
        approver: DeviceId,
        code: &DeviceCode,
        now: SystemTime,
    ) -> Result<(SurfaceRef, String), ControlError> {
        let Some(code_hash) = code_digest(code.expose()) else {
            return Err(ControlError::invalid(
                "a code is the 8 letters and digits the computer shows",
            ));
        };
        let now_ms = timestamp_millis(now)?;
        let path = Arc::clone(&self.path);
        blocking(move || {
            let mut connection = open_connection(&path)?;
            let transaction = connection
                .transaction_with_behavior(TransactionBehavior::Immediate)
                .map_err(sqlite_error)?;
            remove_expired(&transaction, now_ms)?;
            let misses = transaction
                .query_row(
                    "SELECT COUNT(*) FROM device_request_misses WHERE principal_id = ?1",
                    [principal_id.to_string()],
                    |row| row.get::<_, i64>(0),
                )
                .map_err(sqlite_error)?;
            if misses >= MAX_MISSES {
                return Err(ControlError::conflict(
                    "too many codes were wrong; wait five minutes and try again",
                ));
            }
            let waiting = transaction
                .query_row(
                    "SELECT request_id FROM device_requests WHERE code_hash = ?1",
                    [code_hash.as_slice()],
                    |row| row.get::<_, String>(0),
                )
                .optional()
                .map_err(sqlite_error)?;
            let Some(request_id) = waiting else {
                transaction
                    .execute(
                        "INSERT INTO device_request_misses (principal_id, missed_at_ms)
                         VALUES (?1, ?2)",
                        params![principal_id.to_string(), now_ms],
                    )
                    .map_err(sqlite_error)?;
                transaction.commit().map_err(sqlite_error)?;
                return Err(ControlError::not_found(
                    "no computer is waiting with this code; check it, or start again on the computer",
                ));
            };
            let request_id: Uuid = request_id.parse().map_err(id_error)?;
            let request = stored(&transaction, request_id)?
                .ok_or_else(|| ControlError::store("a found device request vanished"))?;
            if request
                .principal_id
                .is_some_and(|approved| approved != principal_id)
            {
                return Err(ControlError::conflict(
                    "another account approved this computer",
                ));
            }
            check_sign_in(
                &transaction,
                principal_id,
                &request.surface,
                &request.registration,
            )?;
            transaction
                .execute(
                    "UPDATE device_requests SET principal_id = ?2, approver_device_id = ?3
                     WHERE request_id = ?1",
                    params![
                        request_id.to_string(),
                        principal_id.to_string(),
                        approver.to_string()
                    ],
                )
                .map_err(sqlite_error)?;
            transaction.commit().map_err(sqlite_error)?;
            Ok((request.surface, request.registration.label))
        })
        .await
    }

    /// Collects a request for the holder of its key: signs the computer in
    /// once an app approved it. Each collection issues a fresh credential
    /// for the same device until the request expires, as a repeated sign-in
    /// does, so a lost answer is collected again.
    pub(crate) async fn collect_device_request(
        &self,
        request_id: Uuid,
        proof: String,
        now: SystemTime,
    ) -> Result<Collected, ControlError> {
        let now_ms = timestamp_millis(now)?;
        let path = Arc::clone(&self.path);
        blocking(move || {
            let mut connection = open_connection(&path)?;
            let transaction = connection
                .transaction_with_behavior(TransactionBehavior::Immediate)
                .map_err(sqlite_error)?;
            remove_expired(&transaction, now_ms)?;
            let Some(request) = stored(&transaction, request_id)? else {
                transaction.commit().map_err(sqlite_error)?;
                return Err(ControlError::not_found(
                    "this request expired or never existed; ask again",
                ));
            };
            if !proves(&request.registration.key, request_id, &proof) {
                return Err(ControlError::authentication_failed());
            }
            let Some(principal_id) = request.principal_id else {
                transaction.commit().map_err(sqlite_error)?;
                return Ok(Collected::Waiting {
                    expires_at_ms: request.expires_at_ms,
                });
            };
            let device = sign_in_device(
                &transaction,
                principal_id,
                &request.surface,
                &request.registration,
                now_ms,
            )?;
            transaction.commit().map_err(sqlite_error)?;
            Ok(Collected::SignedIn(principal_id, device))
        })
        .await
    }
}

#[cfg(test)]
mod tests;
