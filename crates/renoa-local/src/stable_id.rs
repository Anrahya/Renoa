//! Stable identity derivation for operation-keyed writes.
//!
//! One primitive serves every derivation so a caller cannot invent its own
//! hash, and each caller prefixes its own domain string so identities from
//! different operations cannot collide. The coordinator derives its own
//! identities the same way, in a crate this one does not depend on; a pinned
//! vector here and there keeps the two equal.

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

#[cfg(test)]
mod tests {
    use uuid::Uuid;

    use super::stable_id;

    /// The coordinator pins the same vector for its `derived_id`, so neither
    /// copy can change alone: identities already stored keep the values they
    /// were derived under.
    #[test]
    fn a_stable_identity_is_the_pinned_digest_of_its_domain_value() {
        assert_eq!(
            stable_id("renoa.test.vector.v1:pinned"),
            Uuid::parse_str("21c4becb-66ac-d5f6-757c-8bef3a727439").expect("uuid")
        );
    }
}
