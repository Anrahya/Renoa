use std::fmt;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::{ControlError, DeviceId};

const SECRET_BYTES: usize = 32;
const SECRET_HEX_LENGTH: usize = SECRET_BYTES * 2;
const HEX: &[u8; 16] = b"0123456789abcdef";

macro_rules! secret_type {
    ($name:ident, $domain:literal) => {
        #[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
        #[serde(transparent)]
        pub struct $name(String);

        impl $name {
            pub(crate) fn generate() -> Result<Self, ControlError> {
                random_secret().map(Self)
            }

            pub(crate) fn digest(&self) -> Option<[u8; 32]> {
                secret_digest($domain, &self.0)
            }

            #[must_use]
            pub fn expose(&self) -> &str {
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

secret_type!(EnrollmentToken, b"renoa enrollment v1\0");
secret_type!(DeviceCredential, b"renoa device credential v1\0");
secret_type!(PasskeyBootstrapToken, b"renoa passkey bootstrap v1\0");
secret_type!(ConnectionTicket, b"renoa browser connection ticket v1\0");

impl DeviceCredential {
    pub(crate) fn from_encoded(value: String) -> Option<Self> {
        secret_digest(b"renoa device credential v1\0", &value).map(|_| Self(value))
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DeviceCredentials {
    pub device_id: DeviceId,
    pub credential: DeviceCredential,
}

fn random_secret() -> Result<String, ControlError> {
    let mut bytes = [0_u8; SECRET_BYTES];
    getrandom::fill(&mut bytes).map_err(|error| {
        ControlError::store(format!("secure random generation failed: {error}"))
    })?;
    let mut encoded = String::with_capacity(SECRET_HEX_LENGTH);
    for byte in bytes {
        encoded.push(char::from(HEX[usize::from(byte >> 4)]));
        encoded.push(char::from(HEX[usize::from(byte & 0x0f)]));
    }
    Ok(encoded)
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
