//! Stable identity derivation for operation-keyed writes.
//!
//! One primitive serves every derivation so a caller cannot invent its own
//! hash, and each caller prefixes its own domain string so identities from
//! different operations cannot collide.

use sha2::{Digest as _, Sha256};
use uuid::Uuid;

/// Derives a stable identity from one domain-separated value.
#[must_use]
pub(crate) fn stable_id(value: &str) -> Uuid {
    let digest = Sha256::digest(value);
    let mut bytes = [0; 16];
    bytes.copy_from_slice(&digest[..16]);
    Uuid::from_bytes(bytes)
}
