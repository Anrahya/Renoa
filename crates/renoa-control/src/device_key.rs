//! The public key a signed-in app registers, and the proofs it signs with the
//! private half. The private key never reaches the coordinator.

use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use ring::signature::{ECDSA_P256_SHA256_ASN1, UnparsedPublicKey};
use uuid::Uuid;

use crate::{ControlError, DeviceId};

const POINT_BYTES: usize = 65;
const UNCOMPRESSED_POINT_TAG: u8 = 0x04;
/// A DER P-256 signature is at most 72 bytes, which is 96 base64url characters.
const MAX_SIGNATURE_CHARS: usize = 96;
const MAX_LABEL_CHARS: usize = 64;

/// A P-256 public key as the 65-byte uncompressed point.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct DevicePublicKey([u8; POINT_BYTES]);

impl DevicePublicKey {
    pub(crate) fn parse(encoded: &str) -> Result<Self, ControlError> {
        URL_SAFE_NO_PAD
            .decode(encoded)
            .ok()
            .and_then(|bytes| Self::from_bytes(&bytes))
            .ok_or_else(|| {
                ControlError::invalid(
                    "device publicKey must be a 65-byte uncompressed P-256 point \
                     (starting 0x04), base64url without padding",
                )
            })
    }

    pub(crate) fn from_bytes(bytes: &[u8]) -> Option<Self> {
        let point: [u8; POINT_BYTES] = bytes.try_into().ok()?;
        (point[0] == UNCOMPRESSED_POINT_TAG).then_some(Self(point))
    }

    pub(crate) const fn as_bytes(&self) -> &[u8; POINT_BYTES] {
        &self.0
    }

    /// Whether `signature` is this key's ECDSA P-256 SHA-256 signature, in
    /// ASN.1 DER form, over `message`. A point that is not on the curve
    /// verifies nothing.
    pub(crate) fn verify(&self, message: &[u8], signature: &DeviceSignature) -> bool {
        UnparsedPublicKey::new(&ECDSA_P256_SHA256_ASN1, self.0)
            .verify(message, &signature.0)
            .is_ok()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct DeviceSignature(Vec<u8>);

impl DeviceSignature {
    pub(crate) fn parse(encoded: &str) -> Option<Self> {
        if encoded.len() > MAX_SIGNATURE_CHARS {
            return None;
        }
        URL_SAFE_NO_PAD.decode(encoded).ok().map(Self)
    }
}

/// What an app asks to register: its public key and a name the owner will
/// recognize in a list of devices.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct DeviceRegistration {
    pub(crate) key: DevicePublicKey,
    pub(crate) label: String,
}

impl DeviceRegistration {
    pub(crate) fn parse(public_key: &str, label: &str) -> Result<Self, ControlError> {
        let label = label.trim();
        if label.is_empty()
            || label.chars().count() > MAX_LABEL_CHARS
            || label.chars().any(char::is_control)
        {
            return Err(ControlError::invalid(
                "device label must be 1-64 printable characters",
            ));
        }
        Ok(Self {
            key: DevicePublicKey::parse(public_key)?,
            label: label.to_owned(),
        })
    }
}

/// Checks a sign-in's `deviceProof` against the key its ceremony was started
/// for. A ceremony that registers a device needs the proof; a browser
/// ceremony must not carry one.
pub(crate) fn check_registration_proof(
    ceremony_id: Uuid,
    registration: Option<&DeviceRegistration>,
    proof: Option<&str>,
) -> Result<(), ControlError> {
    match (registration, proof) {
        (None, None) => Ok(()),
        (Some(registration), Some(proof)) => {
            let verified = DeviceSignature::parse(proof).is_some_and(|signature| {
                registration
                    .key
                    .verify(&registration_message(ceremony_id), &signature)
            });
            if verified {
                Ok(())
            } else {
                Err(ControlError::invalid(
                    "deviceProof is not the registered key's signature over \
                     \"renoa device registration v1\\n<ceremonyId>\"",
                ))
            }
        }
        (Some(_), None) => Err(ControlError::invalid(
            "deviceProof is required: this ceremony signs in a device",
        )),
        (None, Some(_)) => Err(ControlError::invalid(
            "deviceProof was sent but this ceremony registers no device",
        )),
    }
}

/// The message a connection signs to prove it holds the device key. The nonce
/// is the coordinator's per-connection challenge.
pub(crate) fn proof_message(device_id: DeviceId, nonce: &str) -> Vec<u8> {
    format!("renoa device proof v1\n{device_id}\n{nonce}").into_bytes()
}

/// The message a sign-in signs to prove it holds the key being registered.
pub(crate) fn registration_message(ceremony_id: Uuid) -> Vec<u8> {
    format!("renoa device registration v1\n{ceremony_id}").into_bytes()
}

#[cfg(test)]
mod tests;
