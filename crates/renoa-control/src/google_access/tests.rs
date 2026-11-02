use std::{
    path::Path,
    time::{Duration, SystemTime},
};

use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use ring::signature::{RSA_PKCS1_2048_8192_SHA256, RsaKeyPair, UnparsedPublicKey};

use super::{AccessToken, GoogleCredentials, pkcs8_der};

/// A key generated for these tests only; it signs nothing anywhere else.
const TEST_KEY: &str = include_str!("../../tests/support/push-test-key.pem");

fn service_account_json(private_key: &str) -> String {
    serde_json::json!({
        "type": "service_account",
        "project_id": "renoa-test",
        "private_key_id": "test-key-1",
        "private_key": private_key,
        "client_email": "push@renoa-test.iam.gserviceaccount.com",
        "client_id": "1",
        "token_uri": "https://oauth2.googleapis.com/token",
    })
    .to_string()
}

/// Lines of the key's base64 body, any of which would show the key leaked.
fn key_lines() -> Vec<&'static str> {
    TEST_KEY
        .lines()
        .filter(|line| !line.starts_with("-----"))
        .collect()
}

#[test]
fn a_service_account_signs_an_assertion_the_token_endpoint_can_verify() {
    let (credentials, project_id) =
        GoogleCredentials::from_service_account_json(&service_account_json(TEST_KEY))
            .expect("load the test service account");
    assert_eq!(project_id, "renoa-test");
    let issued = SystemTime::UNIX_EPOCH + std::time::Duration::from_hours(500_000);
    let assertion = credentials
        .source
        .assertion("https://oauth2.googleapis.com/token", issued)
        .expect("sign the assertion");

    let parts: Vec<&str> = assertion.split('.').collect();
    assert_eq!(parts.len(), 3);
    let public_key =
        RsaKeyPair::from_pkcs8(&pkcs8_der(TEST_KEY).expect("test key DER")).expect("test key");
    UnparsedPublicKey::new(&RSA_PKCS1_2048_8192_SHA256, public_key.public().as_ref())
        .verify(
            format!("{}.{}", parts[0], parts[1]).as_bytes(),
            &URL_SAFE_NO_PAD.decode(parts[2]).expect("signature"),
        )
        .expect("the signature verifies with the service account's public key");
    let decode = |part: &str| -> serde_json::Value {
        serde_json::from_slice(&URL_SAFE_NO_PAD.decode(part).expect("base64url")).expect("JSON")
    };
    assert_eq!(
        decode(parts[0]),
        serde_json::json!({"alg": "RS256", "typ": "JWT", "kid": "test-key-1"})
    );
    assert_eq!(
        decode(parts[1]),
        serde_json::json!({
            "iss": "push@renoa-test.iam.gserviceaccount.com",
            "scope": "https://www.googleapis.com/auth/firebase.messaging",
            "aud": "https://oauth2.googleapis.com/token",
            "iat": 1_800_000_000_u64,
            "exp": 1_800_003_600_u64,
        })
    );
}

#[test]
fn debug_output_shows_no_key_material_and_no_access_token() {
    let (credentials, _) =
        GoogleCredentials::from_service_account_json(&service_account_json(TEST_KEY))
            .expect("load the test service account");
    let shown = format!(
        "{credentials:?} {:?}",
        AccessToken("ya29.test-access-token".to_owned())
    );
    for line in key_lines() {
        assert!(!shown.contains(line), "{shown}");
    }
    assert!(!shown.contains("ya29.test-access-token"), "{shown}");
    assert!(shown.contains("[REDACTED]"), "{shown}");
}

#[test]
fn a_damaged_key_is_refused_without_repeating_it() {
    let lines = key_lines();
    let damaged = format!(
        "-----BEGIN PRIVATE KEY-----\n{}\n-----END PRIVATE KEY-----\n",
        lines[..lines.len() / 2].join("\n")
    );
    for json in [
        service_account_json(&damaged),
        service_account_json("not a key"),
        service_account_json(TEST_KEY).replacen("service_account", "external_account", 1),
        service_account_json(TEST_KEY).replacen("renoa-test", "Renoa Test", 1),
        format!("{{\"private_key\": {TEST_KEY:?}"),
    ] {
        let Err(error) = GoogleCredentials::from_service_account_json(&json) else {
            panic!("a damaged service account loads");
        };
        let message = error.to_string();
        for line in &lines {
            assert!(!message.contains(line), "{message}");
        }
    }
}

/// Push settings loaded from a private key file of the test key in
/// `directory`. Only this module may write key material, so other modules'
/// tests get their settings here.
pub(crate) fn push_settings(directory: &Path) -> crate::PushSettings {
    let path = directory.join("fcm-service-account.json");
    std::fs::write(&path, service_account_json(TEST_KEY)).expect("write the key file");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600))
            .expect("make the key file private");
    }
    crate::PushSettings::from_service_account_file(&path).expect("load the key file")
}

#[test]
fn a_device_gap_longer_than_a_day_is_held_to_a_day() {
    let directory = tempfile::tempdir().expect("temporary directory");
    let settings = push_settings(directory.path());
    assert_eq!(
        settings.clone().with_device_gap(Duration::MAX).device_gap(),
        Duration::from_hours(24)
    );
    assert_eq!(
        settings
            .with_device_gap(Duration::from_secs(3))
            .device_gap(),
        Duration::from_secs(3)
    );
}

/// Key material is named only in this module, so no other module can parse,
/// hold or log it. A file elsewhere that needs it fails this test.
#[test]
fn no_other_module_handles_key_material() {
    let source = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut pending = vec![source.clone()];
    let mut offenders = Vec::new();
    while let Some(directory) = pending.pop() {
        for entry in std::fs::read_dir(&directory).expect("read source directory") {
            let path = entry.expect("source entry").path();
            if path.is_dir() {
                pending.push(path);
                continue;
            }
            let relative = path.strip_prefix(&source).expect("inside src");
            if relative.starts_with("google_access") || relative == Path::new("google_access.rs") {
                continue;
            }
            if path.extension().is_none_or(|extension| extension != "rs") {
                continue;
            }
            let text = std::fs::read_to_string(&path).expect("read source file");
            for name in [
                "RsaKeyPair",
                "rsa::",
                "from_pkcs8",
                "private_key",
                "PRIVATE KEY",
            ] {
                if text.contains(name) {
                    offenders.push(format!("{} names {name}", relative.display()));
                }
            }
        }
    }
    assert!(offenders.is_empty(), "{offenders:?}");
}
