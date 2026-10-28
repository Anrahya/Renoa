use uuid::Uuid;

use super::{
    DevicePublicKey, DeviceRegistration, DeviceSignature, check_registration_proof, proof_message,
    registration_message,
};
use crate::DeviceId;

// A key pair and two signatures made outside this crate with
// `openssl dgst -sha256 -sign`, so a client in another language can check that
// it builds the same messages and signature form.
const VECTOR_PUBLIC_KEY: &str =
    "BEFC5QCFE_LZheDMR2lWsqSFcKXGmLzushLPtXThJVeqASyH5pnjQFelgVnW4rcU5iGrCF77DrbWES30UVNgswU";
const VECTOR_DEVICE_ID: u128 = 0xd1;
const VECTOR_NONCE: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";
const VECTOR_PROOF: &str = "MEUCIAnitA2CIfhmPWOB6eOIiJM_Z74hBIIyaP9nxLfrHoibAiEAlm4ly-JV0fkETPZSIdyS8Q0Xk9NkXWmvGYLnnjViCeA";
const VECTOR_CEREMONY_ID: u128 = 0xc1;
const VECTOR_REGISTRATION_PROOF: &str = "MEUCIGTBknvC6AVKrU-9KczMBrhfuFPvQsETvh3tXWR-Jm5_AiEAw054Y_wN-AB-Iw5JllHACXM-4__CGQLWZn6FoU6ccj0";

fn vector_key() -> DevicePublicKey {
    DevicePublicKey::parse(VECTOR_PUBLIC_KEY).expect("vector public key")
}

fn signature(encoded: &str) -> DeviceSignature {
    DeviceSignature::parse(encoded).expect("vector signature")
}

#[test]
fn the_checked_in_vector_verifies_both_messages() {
    let device_id = DeviceId::from_uuid(Uuid::from_u128(VECTOR_DEVICE_ID));
    assert_eq!(
        String::from_utf8(proof_message(device_id, VECTOR_NONCE)).expect("utf-8"),
        format!("renoa device proof v1\n00000000-0000-0000-0000-0000000000d1\n{VECTOR_NONCE}")
    );
    assert!(vector_key().verify(
        &proof_message(device_id, VECTOR_NONCE),
        &signature(VECTOR_PROOF)
    ));

    let ceremony_id = Uuid::from_u128(VECTOR_CEREMONY_ID);
    assert_eq!(
        String::from_utf8(registration_message(ceremony_id)).expect("utf-8"),
        "renoa device registration v1\n00000000-0000-0000-0000-0000000000c1"
    );
    assert!(vector_key().verify(
        &registration_message(ceremony_id),
        &signature(VECTOR_REGISTRATION_PROOF)
    ));
}

#[test]
fn a_signature_covers_exactly_its_message() {
    let device_id = DeviceId::from_uuid(Uuid::from_u128(VECTOR_DEVICE_ID));
    let proof = signature(VECTOR_PROOF);
    let other_nonce = VECTOR_NONCE.replace('0', "1");
    assert!(!vector_key().verify(&proof_message(device_id, &other_nonce), &proof));
    assert!(!vector_key().verify(
        &proof_message(DeviceId::from_uuid(Uuid::from_u128(2)), VECTOR_NONCE),
        &proof
    ));
    // A registration signature cannot stand in for a connection proof.
    assert!(!vector_key().verify(
        &proof_message(device_id, VECTOR_NONCE),
        &signature(VECTOR_REGISTRATION_PROOF)
    ));
}

#[test]
fn only_an_unpadded_uncompressed_point_is_a_key() {
    let compressed = format!("A{}", &VECTOR_PUBLIC_KEY[1..44]);
    let wrong_tag = format!("A{}", &VECTOR_PUBLIC_KEY[1..]);
    let padded = format!("{VECTOR_PUBLIC_KEY}=");
    let standard = VECTOR_PUBLIC_KEY.replace('-', "+").replace('_', "/");
    for rejected in [
        "",
        "not base64!",
        &compressed,
        &wrong_tag,
        &padded,
        &standard,
    ] {
        assert!(
            DevicePublicKey::parse(rejected).is_err(),
            "{rejected:?} must not parse as a device key"
        );
    }
    assert!(DevicePublicKey::parse(VECTOR_PUBLIC_KEY).is_ok());
}

#[test]
fn a_point_off_the_curve_verifies_nothing() {
    let mut point = [0_u8; 65];
    point[0] = 0x04;
    let key = DevicePublicKey::from_bytes(&point).expect("shape is valid");
    assert!(!key.verify(
        &registration_message(Uuid::from_u128(VECTOR_CEREMONY_ID)),
        &signature(VECTOR_REGISTRATION_PROOF)
    ));
}

#[test]
fn a_signature_is_bounded_and_unpadded() {
    assert!(DeviceSignature::parse(&"A".repeat(97)).is_none());
    assert!(DeviceSignature::parse(&format!("{VECTOR_PROOF}=")).is_none());
    assert!(DeviceSignature::parse("***").is_none());
}

#[test]
fn a_label_is_trimmed_printable_and_bounded() {
    let registration = DeviceRegistration::parse(VECTOR_PUBLIC_KEY, "  Pixel 9  ").expect("label");
    assert_eq!(registration.label, "Pixel 9");
    for rejected in ["", "   ", "line\nbreak", &"x".repeat(65)] {
        assert!(
            DeviceRegistration::parse(VECTOR_PUBLIC_KEY, rejected).is_err(),
            "{rejected:?} must not be a device label"
        );
    }
    assert!(DeviceRegistration::parse(VECTOR_PUBLIC_KEY, &"x".repeat(64)).is_ok());
}

#[test]
fn a_ceremony_accepts_exactly_the_proof_it_needs() {
    let ceremony_id = Uuid::from_u128(VECTOR_CEREMONY_ID);
    let registration = DeviceRegistration::parse(VECTOR_PUBLIC_KEY, "Pixel 9").expect("device");

    assert!(
        check_registration_proof(
            ceremony_id,
            Some(&registration),
            Some(VECTOR_REGISTRATION_PROOF)
        )
        .is_ok()
    );
    assert!(check_registration_proof(ceremony_id, None, None).is_ok());

    // A device ceremony without a proof, or a browser ceremony with one.
    assert!(check_registration_proof(ceremony_id, Some(&registration), None).is_err());
    assert!(check_registration_proof(ceremony_id, None, Some(VECTOR_REGISTRATION_PROOF)).is_err());

    // A proof made for another ceremony, a connection proof, and an unparsable one.
    let other_ceremony = Uuid::from_u128(VECTOR_CEREMONY_ID + 1);
    for (ceremony, rejected) in [
        (other_ceremony, VECTOR_REGISTRATION_PROOF),
        (ceremony_id, VECTOR_PROOF),
        (ceremony_id, "***"),
    ] {
        assert!(
            check_registration_proof(ceremony, Some(&registration), Some(rejected)).is_err(),
            "{rejected:?} must not prove ceremony {ceremony}"
        );
    }
}
