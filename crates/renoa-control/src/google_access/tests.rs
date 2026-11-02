use std::{
    collections::VecDeque,
    path::Path,
    sync::{Arc, Mutex},
    time::{Duration, SystemTime},
};

use axum::{Router, extract::State, http::StatusCode};
use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use ring::signature::{RSA_PKCS1_2048_8192_SHA256, RsaKeyPair, UnparsedPublicKey};
use serde_json::{Value, json};
use tokio::net::TcpListener;
use url::Url;

use super::{
    AccessError, AccessToken, AccessTokens, GoogleCredentials, REFRESH_MARGIN, UNAVAILABLE_HOLD,
    pkcs8_der,
};

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

/// A token endpoint that gives each request the next of `answers`, and the
/// count of requests it has had.
async fn token_endpoint(answers: Vec<(u16, Value)>) -> (Url, Arc<Mutex<usize>>) {
    struct Stub {
        answers: Mutex<VecDeque<(u16, Value)>>,
        asked: Arc<Mutex<usize>>,
    }
    async fn answer(State(stub): State<Arc<Stub>>) -> (StatusCode, String) {
        *stub.asked.lock().expect("count") += 1;
        let (status, body) = stub
            .answers
            .lock()
            .expect("answers")
            .pop_front()
            .expect("an answer for every request");
        (
            StatusCode::from_u16(status).expect("status"),
            body.to_string(),
        )
    }
    let asked = Arc::default();
    let stub = Arc::new(Stub {
        answers: Mutex::new(answers.into()),
        asked: Arc::clone(&asked),
    });
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("stub listener");
    let endpoint = Url::parse(&format!(
        "http://{}/token",
        listener.local_addr().expect("address")
    ))
    .expect("stub endpoint");
    let app = Router::new().fallback(answer).with_state(stub);
    tokio::spawn(async move { axum::serve(listener, app).await.expect("serve the stub") });
    (endpoint, asked)
}

fn access_tokens(endpoint: Url) -> AccessTokens {
    let (credentials, _) =
        GoogleCredentials::from_service_account_json(&service_account_json(TEST_KEY))
            .expect("load the test service account");
    AccessTokens::new(
        credentials.with_token_endpoint(endpoint),
        reqwest::Client::new(),
    )
}

fn granted(token: &str, expires_in: u64) -> (u16, Value) {
    (
        200,
        json!({"access_token": token, "expires_in": expires_in}),
    )
}

/// The token `bearer` gives, as text.
async fn bearer(tokens: &AccessTokens) -> Result<String, AccessError> {
    tokens.bearer().await.map(|token| token.0)
}

#[tokio::test]
async fn an_unavailable_token_endpoint_is_asked_again_only_after_a_second() {
    let (endpoint, asked) = token_endpoint(vec![(503, json!({})), (503, json!({}))]).await;
    let tokens = access_tokens(endpoint);
    assert_eq!(bearer(&tokens).await, Err(AccessError::Unavailable));
    assert_eq!(*asked.lock().expect("count"), 1);

    // Only the clock moves while it is paused, so nothing else can ask.
    tokio::time::pause();
    tokio::time::advance(
        UNAVAILABLE_HOLD
            .checked_sub(Duration::from_millis(1))
            .expect("the hold is longer than a millisecond"),
    )
    .await;
    assert_eq!(bearer(&tokens).await, Err(AccessError::Unavailable));
    assert_eq!(*asked.lock().expect("count"), 1, "inside the hold");
    tokio::time::advance(Duration::from_millis(1)).await;
    tokio::time::resume();
    assert_eq!(bearer(&tokens).await, Err(AccessError::Unavailable));
    assert_eq!(*asked.lock().expect("count"), 2, "after the hold");
}

#[tokio::test]
async fn a_token_is_refreshed_its_margin_before_a_lifetime_of_at_most_an_hour() {
    let hour = Duration::from_hours(1);
    for (expires_in, lifetime) in [
        (3599, Duration::from_secs(3599)),
        (7200, hour),
        (u64::MAX, hour),
    ] {
        let (endpoint, asked) = token_endpoint(vec![
            granted("ya29.one", expires_in),
            granted("ya29.two", 3599),
        ])
        .await;
        let tokens = access_tokens(endpoint);
        assert_eq!(bearer(&tokens).await.as_deref(), Ok("ya29.one"));

        tokio::time::pause();
        // A second short of the refresh, as the pause came just after the
        // token was cached.
        let refresh = lifetime
            .checked_sub(REFRESH_MARGIN)
            .expect("a lifetime longer than the margin");
        tokio::time::advance(
            refresh
                .checked_sub(Duration::from_secs(1))
                .expect("a refresh later than a second"),
        )
        .await;
        assert_eq!(
            bearer(&tokens).await.as_deref(),
            Ok("ya29.one"),
            "{expires_in}: cached until its refresh"
        );
        tokio::time::advance(Duration::from_secs(1)).await;
        tokio::time::resume();
        assert_eq!(
            bearer(&tokens).await.as_deref(),
            Ok("ya29.two"),
            "{expires_in}: refreshed at {refresh:?}"
        );
        assert_eq!(*asked.lock().expect("count"), 2);
    }
}
