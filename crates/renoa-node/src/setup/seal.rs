//! Sealing a setup notice to one device's key.
//!
//! The envelope is `1 || ephemeral public key (65) || nonce (12) ||
//! AES-256-GCM ciphertext and tag`, base64url without padding. The key is
//! HKDF-SHA256 over the P-256 ECDH secret of a fresh ephemeral key and the
//! device's key, with [`DOMAIN`] as salt and both public keys as info. The
//! notice id and the device id are associated data, so a notice opens only as
//! itself and only on its device. `tests/setup-notice-vector.json` is the
//! envelope's fixed vector for other clients.

use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use ring::{
    aead::{AES_256_GCM, Aad, LessSafeKey, NONCE_LEN, Nonce, UnboundKey},
    agreement::{ECDH_P256, EphemeralPrivateKey, UnparsedPublicKey, agree_ephemeral},
    hkdf::{HKDF_SHA256, Salt},
    rand::{SecureRandom as _, SystemRandom},
};
use uuid::Uuid;

/// Names the envelope wherever its bytes are bound: salt and associated data.
pub(crate) const DOMAIN: &[u8] = b"renoa.setup-notice.v1";
const VERSION: u8 = 1;

/// Why a notice could not be sealed to a device.
#[derive(Debug, thiserror::Error)]
#[error("the device's notice key is not a usable P-256 key")]
pub(crate) struct Unsealable;

/// Seals `plaintext` for `device_id`'s key `recipient`, as notice `notice_id`.
pub(crate) fn seal(
    recipient: &[u8; 65],
    notice_id: Uuid,
    device_id: Uuid,
    plaintext: &[u8],
) -> Result<String, Unsealable> {
    let random = SystemRandom::new();
    let ephemeral = EphemeralPrivateKey::generate(&ECDH_P256, &random).map_err(|_| Unsealable)?;
    let ephemeral_public = ephemeral.compute_public_key().map_err(|_| Unsealable)?;
    let ephemeral_public = ephemeral_public.as_ref().to_vec();
    let mut info = ephemeral_public.clone();
    info.extend_from_slice(recipient);
    let key = agree_ephemeral(
        ephemeral,
        &UnparsedPublicKey::new(&ECDH_P256, recipient),
        |secret| {
            let info = [info.as_slice()];
            let prk = Salt::new(HKDF_SHA256, DOMAIN).extract(secret);
            let okm = prk.expand(&info, &AES_256_GCM).map_err(|_| Unsealable)?;
            Ok::<_, Unsealable>(LessSafeKey::new(UnboundKey::from(okm)))
        },
    )
    .map_err(|_| Unsealable)??;
    let mut nonce = [0_u8; NONCE_LEN];
    random.fill(&mut nonce).map_err(|_| Unsealable)?;
    let mut sealed = plaintext.to_vec();
    key.seal_in_place_append_tag(
        Nonce::assume_unique_for_key(nonce),
        Aad::from(associated_data(notice_id, device_id)),
        &mut sealed,
    )
    .map_err(|_| Unsealable)?;
    let mut envelope = Vec::with_capacity(1 + ephemeral_public.len() + NONCE_LEN + sealed.len());
    envelope.push(VERSION);
    envelope.extend_from_slice(&ephemeral_public);
    envelope.extend_from_slice(&nonce);
    envelope.extend_from_slice(&sealed);
    Ok(URL_SAFE_NO_PAD.encode(envelope))
}

/// The bytes a notice is bound to: [`DOMAIN`], then the notice and device ids.
pub(crate) fn associated_data(notice_id: Uuid, device_id: Uuid) -> Vec<u8> {
    let mut data = DOMAIN.to_vec();
    data.extend_from_slice(notice_id.as_bytes());
    data.extend_from_slice(device_id.as_bytes());
    data
}

#[cfg(test)]
#[path = "seal/tests.rs"]
pub(super) mod tests;
