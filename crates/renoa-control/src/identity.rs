//! The coordinator's secrets: how each is generated and digested. Only a
//! digest is ever stored, under a domain that names the secret's kind, so a
//! digest of one kind never matches another.

use std::fmt;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::{ConnectionTicket, ControlError, DeviceCredential, EnrollmentToken};

const SECRET_BYTES: usize = 32;
const SECRET_HEX_LENGTH: usize = SECRET_BYTES * 2;
const HEX: &[u8; 16] = b"0123456789abcdef";

/// A 256-bit random secret, carried as lowercase hex.
pub(crate) trait Secret: Sized {
    /// The digest's domain separator, unique to this kind of secret.
    const DOMAIN: &'static [u8];

    fn wrap(encoded: String) -> Self;

    fn encoded(&self) -> &str;

    fn generate() -> Result<Self, ControlError> {
        random_secret().map(Self::wrap)
    }

    /// The stored form of the secret, or `None` when it is not 64 hex
    /// characters and so was never issued.
    fn digest(&self) -> Option<[u8; 32]> {
        secret_digest(Self::DOMAIN, self.encoded())
    }

    /// A secret read from a request, when it is well formed.
    fn from_encoded(value: &str) -> Option<Self> {
        secret_digest(Self::DOMAIN, value).map(|_| Self::wrap(value.to_owned()))
    }
}

macro_rules! wire_secret {
    ($name:ident, $domain:literal) => {
        impl Secret for $name {
            const DOMAIN: &'static [u8] = $domain;

            fn wrap(encoded: String) -> Self {
                Self::new(encoded)
            }

            fn encoded(&self) -> &str {
                self.expose()
            }
        }
    };
}

macro_rules! secret_type {
    ($name:ident, $domain:literal) => {
        #[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
        #[serde(transparent)]
        pub struct $name(String);

        impl $name {
            #[must_use]
            pub fn expose(&self) -> &str {
                &self.0
            }
        }

        impl Secret for $name {
            const DOMAIN: &'static [u8] = $domain;

            fn wrap(encoded: String) -> Self {
                Self(encoded)
            }

            fn encoded(&self) -> &str {
                &self.0
            }
        }

        impl fmt::Debug for $name {
            fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str(concat!(stringify!($name), "([REDACTED])"))
            }
        }
    };
}

wire_secret!(EnrollmentToken, b"renoa enrollment v1\0");
wire_secret!(DeviceCredential, b"renoa device credential v1\0");
wire_secret!(ConnectionTicket, b"renoa browser connection ticket v1\0");
secret_type!(PasskeyBootstrapToken, b"renoa passkey bootstrap v1\0");
secret_type!(BrowserSessionToken, b"renoa browser session v1\0");
secret_type!(BrowserPairingToken, b"renoa browser pairing v1\0");
secret_type!(ShareTicket, b"renoa share ticket v1\0");

impl BrowserPairingToken {
    // A retry from the same browser recreates its cookie without storing plaintext.
    // A different nonce cannot redeem an already-claimed pairing grant.
    pub(crate) fn session_for(&self, nonce: &str) -> Option<BrowserSessionToken> {
        self.digest()?;
        secret_digest(b"renoa browser pairing nonce v1\0", nonce)?;
        let key = ring::hmac::Key::new(ring::hmac::HMAC_SHA256, self.0.as_bytes());
        let proof = format!("renoa paired browser session v1\0{nonce}");
        let tag = ring::hmac::sign(&key, proof.as_bytes());
        Some(BrowserSessionToken(encode_secret(tag.as_ref())))
    }
}

/// A fresh 256-bit challenge as lowercase hex. The peer it is sent to may read
/// it; only its freshness matters.
pub(crate) fn random_nonce() -> Result<String, ControlError> {
    random_secret()
}

fn random_secret() -> Result<String, ControlError> {
    let mut bytes = [0_u8; SECRET_BYTES];
    getrandom::fill(&mut bytes).map_err(|error| {
        ControlError::store(format!("secure random generation failed: {error}"))
    })?;
    Ok(encode_secret(&bytes))
}

fn encode_secret(bytes: &[u8]) -> String {
    let mut encoded = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        encoded.push(char::from(HEX[usize::from(byte >> 4)]));
        encoded.push(char::from(HEX[usize::from(byte & 0x0f)]));
    }
    encoded
}

fn secret_digest(domain: &[u8], secret: &str) -> Option<[u8; 32]> {
    if secret.len() != SECRET_HEX_LENGTH || !secret.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return None;
    }
    let mut digest = Sha256::new();
    digest.update(domain);
    digest.update(secret.as_bytes());
    Some(digest.finalize().into())
}
