use aws_lc_rs::{
    aead::{AES_256_GCM, Aad, LessSafeKey, Nonce, UnboundKey},
    agreement::{ECDH_P256, PrivateKey, UnparsedPublicKey, agree},
    encoding::{AsBigEndian as _, EcPrivateKeyBin},
    hkdf::{HKDF_SHA256, Salt},
};
use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use serde::Deserialize;
use uuid::Uuid;

use super::{DOMAIN, associated_data, seal};

/// Opens a notice the way a device does: with its own stored key.
pub(crate) fn open(
    private: &PrivateKey,
    sealed: &str,
    notice_id: Uuid,
    device_id: Uuid,
) -> Option<Vec<u8>> {
    let envelope = URL_SAFE_NO_PAD.decode(sealed).ok()?;
    let (&version, rest) = envelope.split_first()?;
    if version != 1 || rest.len() < 65 + 12 + 16 {
        return None;
    }
    let (ephemeral, rest) = rest.split_at(65);
    let (nonce, ciphertext) = rest.split_at(12);
    let recipient = private.compute_public_key().ok()?;
    let mut info = ephemeral.to_vec();
    info.extend_from_slice(recipient.as_ref());
    let key = agree(
        private,
        UnparsedPublicKey::new(&ECDH_P256, ephemeral),
        (),
        |secret| {
            Salt::new(HKDF_SHA256, DOMAIN)
                .extract(secret)
                .expand(&[&info], &AES_256_GCM)
                .map(|okm| LessSafeKey::new(UnboundKey::from(okm)))
                .map_err(|_| ())
        },
    )
    .ok()?;
    let mut buffer = ciphertext.to_vec();
    key.open_in_place(
        Nonce::try_assume_unique_for_key(nonce).ok()?,
        Aad::from(associated_data(notice_id, device_id)),
        &mut buffer,
    )
    .ok()
    .map(|plaintext| plaintext.to_vec())
}

pub(crate) fn device_key() -> (PrivateKey, [u8; 65]) {
    let private = PrivateKey::generate(&ECDH_P256).expect("device key");
    let public: [u8; 65] = private
        .compute_public_key()
        .expect("public key")
        .as_ref()
        .try_into()
        .expect("uncompressed point");
    (private, public)
}

#[test]
fn a_notice_opens_only_on_its_device_as_itself() {
    let (private, public) = device_key();
    let (notice_id, device_id) = (Uuid::new_v4(), Uuid::new_v4());
    let sealed = seal(&public, notice_id, device_id, b"the link").expect("sealed");
    assert_eq!(
        open(&private, &sealed, notice_id, device_id).as_deref(),
        Some(&b"the link"[..])
    );
    assert_eq!(
        open(&private, &sealed, Uuid::new_v4(), device_id),
        None,
        "another notice"
    );
    assert_eq!(
        open(&private, &sealed, notice_id, Uuid::new_v4()),
        None,
        "another device"
    );
    let (other, _) = device_key();
    assert_eq!(
        open(&other, &sealed, notice_id, device_id),
        None,
        "another key"
    );
    assert_ne!(
        sealed,
        seal(&public, notice_id, device_id, b"the link").expect("sealed again"),
        "every seal uses a fresh key and nonce"
    );
}

#[test]
fn a_key_that_is_no_point_is_refused() {
    let mut point = [7_u8; 65];
    point[0] = 0x04;
    assert!(seal(&point, Uuid::new_v4(), Uuid::new_v4(), b"x").is_err());
}

#[derive(Deserialize)]
struct Vector {
    private_key: String,
    notice_id: Uuid,
    device_id: Uuid,
    sealed: String,
    plaintext: String,
}

/// The fixed vector other clients check their opening against.
#[test]
fn the_checked_in_vector_opens() {
    let vector: Vector =
        serde_json::from_str(include_str!("../../../tests/setup-notice-vector.json"))
            .expect("vector");
    let scalar = URL_SAFE_NO_PAD.decode(&vector.private_key).expect("scalar");
    let private = PrivateKey::from_private_key(&ECDH_P256, &scalar).expect("private key");
    let opened = open(&private, &vector.sealed, vector.notice_id, vector.device_id)
        .expect("the vector opens");
    assert_eq!(String::from_utf8(opened).expect("UTF-8"), vector.plaintext);
}

/// Prints a fresh vector; run by hand with `--ignored --nocapture` to replace
/// `tests/setup-notice-vector.json`.
#[test]
#[ignore = "writes a new vector by hand"]
fn print_a_vector() {
    let (private, public) = device_key();
    let (notice_id, device_id) = (Uuid::new_v4(), Uuid::new_v4());
    let plaintext = r#"{"kind":"authorization","label":"Notion MCP","url":"https://provider.example/authorize?state=one","expires_at_ms":1893456000000}"#;
    let sealed = seal(&public, notice_id, device_id, plaintext.as_bytes()).expect("sealed");
    let scalar: EcPrivateKeyBin<'static> = private.as_be_bytes().expect("scalar");
    println!(
        "{}",
        serde_json::json!({
            "private_key": URL_SAFE_NO_PAD.encode(scalar.as_ref()),
            "public_key": URL_SAFE_NO_PAD.encode(public),
            "notice_id": notice_id,
            "device_id": device_id,
            "sealed": sealed,
            "plaintext": plaintext,
        })
    );
}
