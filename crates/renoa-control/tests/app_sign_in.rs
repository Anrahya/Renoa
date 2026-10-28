//! App sign-in: a passkey plus a device key, proof of the key on every
//! connection, and the owner's list and revocation of devices. Every test runs
//! a real coordinator over a real SQLite file with a software passkey and a
//! real P-256 key.

use std::{path::Path, time::Duration};

use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use futures_util::{SinkExt, StreamExt};
use renoa_control::{
    AndroidApps, ClientMessage, Coordinator, DeviceCredentials, DeviceId, DeviceSummary, ErrorCode,
    JSON_WS_VERSION, PeerIdentity, ServerMessage,
};
use renoa_protocol::{PrincipalId, SurfaceRef};
use reqwest::{StatusCode, header};
use ring::{
    rand::SystemRandom,
    signature::{ECDSA_P256_SHA256_ASN1_SIGNING, EcdsaKeyPair, KeyPair},
};
use serde_json::{Value, json};
use tokio::{net::TcpListener, task::JoinHandle, time::timeout};
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream, connect_async, tungstenite::Message};
use tokio_util::sync::CancellationToken;
use url::Url;
use uuid::Uuid;
use webauthn_authenticator_rs::{AuthenticatorBackend, softpasskey::SoftPasskey};
use webauthn_rs::prelude::{CreationChallengeResponse, RequestChallengeResponse};

type Socket = WebSocketStream<MaybeTlsStream<tokio::net::TcpStream>>;

const WEB_ORIGIN: &str = "http://localhost";
const WAIT: Duration = Duration::from_secs(10);

struct Running {
    coordinator: Coordinator,
    http: String,
    websocket: String,
    database: std::path::PathBuf,
    shutdown: CancellationToken,
    task: JoinHandle<Result<(), renoa_control::ControlError>>,
}

impl Running {
    async fn start(database: &Path) -> Self {
        Self::start_with(database, AndroidApps::default()).await
    }

    async fn start_with(database: &Path, apps: AndroidApps) -> Self {
        let coordinator =
            Coordinator::open_with_android_apps(database, "localhost", WEB_ORIGIN, apps)
                .expect("open coordinator");
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
        let address = listener.local_addr().expect("address");
        let shutdown = CancellationToken::new();
        let task = tokio::spawn({
            let coordinator = coordinator.clone();
            let shutdown = shutdown.clone();
            async move { coordinator.serve(listener, shutdown).await }
        });
        Self {
            coordinator,
            http: format!("http://{address}"),
            websocket: format!("ws://{address}/connect"),
            database: database.to_path_buf(),
            shutdown,
            task,
        }
    }

    async fn stop(self) {
        self.shutdown.cancel();
        self.task
            .await
            .expect("coordinator task")
            .expect("coordinator stops cleanly");
    }

    fn rows(&self, sql: &str) -> i64 {
        rusqlite::Connection::open(&self.database)
            .expect("open database")
            .query_row(sql, [], |row| row.get(0))
            .expect("count rows")
    }

    fn execute(&self, sql: &str) {
        rusqlite::Connection::open(&self.database)
            .expect("open database")
            .execute_batch(sql)
            .expect("run statements");
    }

    async fn post(&self, path: &str, body: &Value) -> (StatusCode, header::HeaderMap, Value) {
        let response = reqwest::Client::new()
            .post(format!("{}{path}", self.http))
            .json(body)
            .send()
            .await
            .expect("identity request");
        let status = response.status();
        let headers = response.headers().clone();
        let body = response.json().await.unwrap_or(Value::Null);
        (status, headers, body)
    }
}

/// An app's P-256 key, kept in the test as the Keystore keeps it in the app.
struct DeviceKey(EcdsaKeyPair);

impl DeviceKey {
    fn generate() -> Self {
        let rng = SystemRandom::new();
        let pkcs8 = EcdsaKeyPair::generate_pkcs8(&ECDSA_P256_SHA256_ASN1_SIGNING, &rng)
            .expect("generate key");
        Self(
            EcdsaKeyPair::from_pkcs8(&ECDSA_P256_SHA256_ASN1_SIGNING, pkcs8.as_ref(), &rng)
                .expect("load key"),
        )
    }

    fn public_key(&self) -> String {
        URL_SAFE_NO_PAD.encode(self.0.public_key().as_ref())
    }

    fn sign(&self, message: &str) -> String {
        let signature = self
            .0
            .sign(&SystemRandom::new(), message.as_bytes())
            .expect("sign");
        URL_SAFE_NO_PAD.encode(signature.as_ref())
    }
}

/// A person with a software passkey, which signs in apps at one origin.
struct Person {
    principal: PrincipalId,
    passkey: SoftPasskey,
    origin: Url,
}

impl Person {
    fn new(principal: u128, origin: &str) -> Self {
        Self {
            principal: PrincipalId::from_uuid(Uuid::from_u128(principal)),
            passkey: SoftPasskey::new(true),
            origin: Url::parse(origin).expect("origin"),
        }
    }

    /// Registers the person's passkey and signs the first app in with it.
    async fn register(
        &mut self,
        server: &Running,
        key: &DeviceKey,
        surface: &str,
        label: &str,
    ) -> DeviceCredentials {
        let bootstrap = server
            .coordinator
            .create_passkey_bootstrap(self.principal, std::time::SystemTime::now() + WAIT)
            .await
            .expect("bootstrap");
        let (status, _, options) = server
            .post(
                "/v1/identity/passkeys/registration/options",
                &json!({
                    "bootstrapToken": bootstrap,
                    "surface": surface,
                    "device": { "publicKey": key.public_key(), "label": label },
                }),
            )
            .await;
        assert_eq!(status, StatusCode::OK, "{options}");
        let ceremony_id = options["ceremonyId"]
            .as_str()
            .expect("ceremony id")
            .to_owned();
        let challenge: CreationChallengeResponse =
            serde_json::from_value(options["options"].clone()).expect("creation options");
        let credential = self
            .passkey
            .perform_register(self.origin.clone(), challenge.public_key, 60_000)
            .expect("register passkey");
        let (status, headers, body) = server
            .post(
                "/v1/identity/passkeys/registration/verify",
                &json!({
                    "ceremonyId": ceremony_id,
                    "credential": credential,
                    "deviceProof": key.sign(&format!("renoa device registration v1\n{ceremony_id}")),
                }),
            )
            .await;
        assert_signed_in(status, &headers, &body)
    }

    async fn start_sign_in(
        &self,
        server: &Running,
        surface: &str,
        device: Option<(&DeviceKey, &str)>,
    ) -> (StatusCode, Value) {
        let mut request = json!({ "principalId": self.principal, "surface": surface });
        if let Some((key, label)) = device {
            request["device"] = json!({ "publicKey": key.public_key(), "label": label });
        }
        let (status, _, body) = server
            .post("/v1/identity/passkeys/authentication/options", &request)
            .await;
        (status, body)
    }

    fn assert(&mut self, options: &Value) -> Value {
        let challenge: RequestChallengeResponse =
            serde_json::from_value(options["options"].clone()).expect("request options");
        let credential = self
            .passkey
            .perform_auth(self.origin.clone(), challenge.public_key, 60_000)
            .expect("passkey assertion");
        serde_json::to_value(credential).expect("assertion JSON")
    }

    /// Runs a whole sign-in with the passkey the person already registered and
    /// returns the answer to `verify` as it came. Starting the ceremony must
    /// succeed: the coordinator says nothing about a key before the passkey.
    async fn try_sign_in(
        &mut self,
        server: &Running,
        key: &DeviceKey,
        surface: &str,
        label: &str,
    ) -> (StatusCode, header::HeaderMap, Value) {
        let (status, options) = self
            .start_sign_in(server, surface, Some((key, label)))
            .await;
        assert_eq!(status, StatusCode::OK, "{options}");
        let ceremony_id = options["ceremonyId"]
            .as_str()
            .expect("ceremony id")
            .to_owned();
        let credential = self.assert(&options);
        server
            .post(
                "/v1/identity/passkeys/authentication/verify",
                &json!({
                    "ceremonyId": ceremony_id,
                    "credential": credential,
                    "deviceProof": key.sign(&format!("renoa device registration v1\n{ceremony_id}")),
                }),
            )
            .await
    }

    /// Signs an app in with the passkey the person already registered.
    async fn sign_in(
        &mut self,
        server: &Running,
        key: &DeviceKey,
        surface: &str,
        label: &str,
    ) -> DeviceCredentials {
        let (status, headers, body) = self.try_sign_in(server, key, surface, label).await;
        assert_signed_in(status, &headers, &body)
    }

    /// A browser sign-in: a one-use ticket and no device.
    async fn browser_ticket(&mut self, server: &Running) -> Value {
        let (status, options) = self.start_sign_in(server, "web", None).await;
        assert_eq!(status, StatusCode::OK, "{options}");
        let credential = self.assert(&options);
        let (status, _, body) = server
            .post(
                "/v1/identity/passkeys/authentication/verify",
                &json!({ "ceremonyId": options["ceremonyId"], "credential": credential }),
            )
            .await;
        assert_eq!(status, StatusCode::OK, "{body}");
        body["connectionTicket"].clone()
    }
}

fn assert_signed_in(
    status: StatusCode,
    headers: &header::HeaderMap,
    body: &Value,
) -> DeviceCredentials {
    assert_eq!(status, StatusCode::OK, "{body}");
    assert!(
        headers.get(header::SET_COOKIE).is_none(),
        "an app is given no cookie"
    );
    assert!(
        body.get("connectionTicket").is_none(),
        "an app is given no ticket"
    );
    serde_json::from_value(body["credentials"].clone()).expect("device credentials")
}

async fn connect(server: &Running) -> Socket {
    connect_async(&server.websocket).await.expect("connect").0
}

async fn send(socket: &mut Socket, message: &ClientMessage) {
    let json = serde_json::to_string(message).expect("serialize");
    socket.send(Message::Text(json.into())).await.expect("send");
}

async fn receive(socket: &mut Socket) -> ServerMessage {
    loop {
        match timeout(WAIT, socket.next())
            .await
            .expect("the coordinator answered")
            .expect("the socket stayed open")
            .expect("frame")
        {
            Message::Text(json) => return serde_json::from_str(&json).expect("server message"),
            Message::Ping(_) | Message::Pong(_) => {}
            other => panic!("unexpected frame {other:?}"),
        }
    }
}

async fn expect_closed(socket: &mut Socket) {
    loop {
        match timeout(WAIT, socket.next()).await {
            Ok(None | Some(Err(_) | Ok(Message::Close(_)))) => return,
            Ok(Some(Ok(Message::Ping(_) | Message::Pong(_)))) => {}
            Ok(Some(Ok(other))) => panic!("expected the socket to close, received {other:?}"),
            Err(elapsed) => panic!("the socket stayed open: {elapsed}"),
        }
    }
}

async fn authenticate(socket: &mut Socket, credentials: &DeviceCredentials) {
    send(
        socket,
        &ClientMessage::Authenticate {
            version: JSON_WS_VERSION,
            credentials: credentials.clone(),
        },
    )
    .await;
}

/// The message a connection signs, built here from the documented format.
fn proof_for(credentials: &DeviceCredentials, nonce: &str) -> String {
    format!("renoa device proof v1\n{}\n{nonce}", credentials.device_id)
}

async fn challenge(socket: &mut Socket) -> String {
    let ServerMessage::DeviceChallenge { nonce } = receive(socket).await else {
        panic!("a key-bound device is challenged");
    };
    assert_eq!(nonce.len(), 64);
    nonce
}

async fn expect_error(socket: &mut Socket, expected: ErrorCode) {
    let ServerMessage::Error { code, .. } = receive(socket).await else {
        panic!("expected an {expected:?} error");
    };
    assert_eq!(code, expected);
}

/// Connects a key-bound device the way an app does.
async fn connect_device(
    server: &Running,
    credentials: &DeviceCredentials,
    key: &DeviceKey,
) -> Socket {
    let mut socket = connect(server).await;
    authenticate(&mut socket, credentials).await;
    let nonce = challenge(&mut socket).await;
    send(
        &mut socket,
        &ClientMessage::DeviceProof {
            signature: key.sign(&proof_for(credentials, &nonce)),
        },
    )
    .await;
    let ServerMessage::Authenticated { version } = receive(&mut socket).await else {
        panic!("a valid proof authenticates");
    };
    assert_eq!(version, JSON_WS_VERSION);
    socket
}

async fn list_devices(socket: &mut Socket, request_id: u64) -> Vec<DeviceSummary> {
    send(socket, &ClientMessage::ListDevices { request_id }).await;
    let ServerMessage::DeviceList {
        request_id: answered,
        devices,
    } = receive(socket).await
    else {
        panic!("expected a device list");
    };
    assert_eq!(answered, request_id);
    devices
}

#[tokio::test]
async fn an_app_signs_in_with_a_passkey_and_connects_only_with_credential_and_key() {
    let files = tempfile::tempdir().expect("temporary directory");
    let database = files.path().join("control.sqlite");
    let server = Running::start(&database).await;
    let mut person = Person::new(1, WEB_ORIGIN);
    let key = DeviceKey::generate();

    let credentials = person
        .register(&server, &key, "android", "  Pixel 9 ")
        .await;

    assert_eq!(server.rows("SELECT COUNT(*) FROM devices"), 1);
    assert_eq!(server.rows("SELECT COUNT(*) FROM device_keys"), 1);
    assert_eq!(server.rows("SELECT COUNT(*) FROM passkeys"), 1);
    assert_eq!(server.rows("SELECT COUNT(*) FROM browser_sessions"), 0);
    assert_eq!(
        server.rows("SELECT COUNT(*) FROM browser_connection_tickets"),
        0
    );
    assert_eq!(server.rows("SELECT COUNT(*) FROM device_sign_ins"), 0);
    let stored: Vec<u8> = rusqlite::Connection::open(&database)
        .expect("open database")
        .query_row("SELECT public_key FROM device_keys", [], |row| row.get(0))
        .expect("stored key");
    assert_eq!(URL_SAFE_NO_PAD.encode(stored), key.public_key());

    // The sign-in survives a coordinator restart before the app first connects.
    server.stop().await;
    let server = Running::start(&database).await;
    let mut socket = connect_device(&server, &credentials, &key).await;
    let devices = list_devices(&mut socket, 1).await;
    assert_eq!(devices.len(), 1);
    assert_eq!(devices[0].device_id, credentials.device_id);
    assert_eq!(devices[0].surface, SurfaceRef::new("android"));
    assert_eq!(devices[0].label, "Pixel 9");
    assert!(devices[0].current);
    server.stop().await;
}

#[tokio::test]
async fn a_bearer_credential_alone_never_authenticates_a_key_bound_device() {
    let files = tempfile::tempdir().expect("temporary directory");
    let server = Running::start(&files.path().join("control.sqlite")).await;
    let mut person = Person::new(1, WEB_ORIGIN);
    let key = DeviceKey::generate();
    let credentials = person.register(&server, &key, "android", "Pixel 9").await;

    // No proof: the next frame is an operation.
    let mut socket = connect(&server).await;
    authenticate(&mut socket, &credentials).await;
    challenge(&mut socket).await;
    send(&mut socket, &ClientMessage::ListTasks { request_id: 1 }).await;
    expect_error(&mut socket, ErrorCode::AuthenticationFailed).await;
    expect_closed(&mut socket).await;

    // A proof by a key the device did not register.
    let mut socket = connect(&server).await;
    authenticate(&mut socket, &credentials).await;
    let nonce = challenge(&mut socket).await;
    let thief = DeviceKey::generate();
    send(
        &mut socket,
        &ClientMessage::DeviceProof {
            signature: thief.sign(&proof_for(&credentials, &nonce)),
        },
    )
    .await;
    expect_error(&mut socket, ErrorCode::AuthenticationFailed).await;
    expect_closed(&mut socket).await;

    // A proof recorded from an earlier connection signs another nonce.
    let mut earlier = connect(&server).await;
    authenticate(&mut earlier, &credentials).await;
    let earlier_nonce = challenge(&mut earlier).await;
    let recorded = key.sign(&proof_for(&credentials, &earlier_nonce));
    send(
        &mut earlier,
        &ClientMessage::DeviceProof {
            signature: recorded.clone(),
        },
    )
    .await;
    assert!(matches!(
        receive(&mut earlier).await,
        ServerMessage::Authenticated { .. }
    ));
    let mut socket = connect(&server).await;
    authenticate(&mut socket, &credentials).await;
    let nonce = challenge(&mut socket).await;
    assert_ne!(
        nonce, earlier_nonce,
        "every connection has its own challenge"
    );
    send(
        &mut socket,
        &ClientMessage::DeviceProof {
            signature: recorded,
        },
    )
    .await;
    expect_error(&mut socket, ErrorCode::AuthenticationFailed).await;
    expect_closed(&mut socket).await;

    // A proof for another device's message, a malformed proof and an oversize one.
    for signature in [
        key.sign(&format!(
            "renoa device proof v1\n{}\n{}",
            Uuid::from_u128(9),
            "0".repeat(64)
        )),
        "not a signature!".to_owned(),
        "A".repeat(4096),
    ] {
        let mut socket = connect(&server).await;
        authenticate(&mut socket, &credentials).await;
        challenge(&mut socket).await;
        send(&mut socket, &ClientMessage::DeviceProof { signature }).await;
        expect_error(&mut socket, ErrorCode::AuthenticationFailed).await;
        expect_closed(&mut socket).await;
    }

    // A proof without having been challenged is not a first frame.
    let mut socket = connect(&server).await;
    send(
        &mut socket,
        &ClientMessage::DeviceProof {
            signature: key.sign("anything"),
        },
    )
    .await;
    expect_error(&mut socket, ErrorCode::InvalidMessage).await;
    expect_closed(&mut socket).await;

    // After authentication a proof is not an operation.
    let mut socket = connect_device(&server, &credentials, &key).await;
    send(
        &mut socket,
        &ClientMessage::DeviceProof {
            signature: key.sign("again"),
        },
    )
    .await;
    expect_error(&mut socket, ErrorCode::InvalidMessage).await;
    server.stop().await;
}

#[tokio::test]
async fn a_device_without_a_key_connects_exactly_as_before() {
    let files = tempfile::tempdir().expect("temporary directory");
    let server = Running::start(&files.path().join("control.sqlite")).await;
    let credentials = enroll_link(&server, 1, "discord").await;

    let mut socket = connect(&server).await;
    authenticate(&mut socket, &credentials).await;
    assert!(matches!(
        receive(&mut socket).await,
        ServerMessage::Authenticated { .. }
    ));
    server.stop().await;
}

/// A bearer-only link such as the Discord surface, enrolled by the operator.
async fn enroll_link(server: &Running, principal: u128, surface: &str) -> DeviceCredentials {
    let token = server
        .coordinator
        .create_enrollment(
            PeerIdentity::Surface {
                principal_id: PrincipalId::from_uuid(Uuid::from_u128(principal)),
                surface: SurfaceRef::new(surface),
            },
            std::time::SystemTime::now() + WAIT,
        )
        .await
        .expect("enrollment");
    let mut socket = connect(server).await;
    send(
        &mut socket,
        &ClientMessage::Enroll {
            version: JSON_WS_VERSION,
            token,
        },
    )
    .await;
    let ServerMessage::Enrolled { credentials, .. } = receive(&mut socket).await else {
        panic!("enrollment returns credentials");
    };
    credentials
}

#[tokio::test]
async fn signing_in_again_with_the_same_key_converges_on_one_device() {
    let files = tempfile::tempdir().expect("temporary directory");
    let server = Running::start(&files.path().join("control.sqlite")).await;
    let mut person = Person::new(1, WEB_ORIGIN);
    let key = DeviceKey::generate();
    // The first response is lost: the app never learns this credential.
    let lost = person.register(&server, &key, "android", "Pixel 9").await;
    let mut old_socket = connect_device(&server, &lost, &key).await;

    let again = person
        .sign_in(&server, &key, "android", "Pixel 9 (new name)")
        .await;
    expect_closed(&mut old_socket).await;

    assert_eq!(
        again.device_id, lost.device_id,
        "the key is the device's identity"
    );
    assert_ne!(again.credential, lost.credential);
    assert_eq!(server.rows("SELECT COUNT(*) FROM devices"), 1);
    assert_eq!(server.rows("SELECT COUNT(*) FROM device_keys"), 1);
    let mut stale = connect(&server).await;
    authenticate(&mut stale, &lost).await;
    expect_error(&mut stale, ErrorCode::AuthenticationFailed).await;
    let mut socket = connect_device(&server, &again, &key).await;
    let devices = list_devices(&mut socket, 1).await;
    assert_eq!(devices.len(), 1);
    assert_eq!(devices[0].label, "Pixel 9 (new name)");
    server.stop().await;
}

#[tokio::test]
async fn a_sign_in_that_replaces_the_credential_ends_a_connection_still_proving_its_key() {
    let files = tempfile::tempdir().expect("temporary directory");
    let server = Running::start(&files.path().join("control.sqlite")).await;
    let mut person = Person::new(1, WEB_ORIGIN);
    let key = DeviceKey::generate();
    let replaced = person.register(&server, &key, "android", "Pixel 9").await;

    // This connection authenticated with the credential and now waits to prove its key.
    let mut straggler = connect(&server).await;
    authenticate(&mut straggler, &replaced).await;
    let nonce = challenge(&mut straggler).await;
    let again = person.sign_in(&server, &key, "android", "Pixel 9").await;
    assert_eq!(again.device_id, replaced.device_id);

    // A correct proof does not bring the replaced credential back.
    send(
        &mut straggler,
        &ClientMessage::DeviceProof {
            signature: key.sign(&proof_for(&replaced, &nonce)),
        },
    )
    .await;
    expect_error(&mut straggler, ErrorCode::AuthenticationFailed).await;
    expect_closed(&mut straggler).await;
    connect_device(&server, &again, &key).await;
    server.stop().await;
}

#[tokio::test]
async fn a_revocation_during_the_key_handshake_ends_the_connection_before_it_is_authenticated() {
    let files = tempfile::tempdir().expect("temporary directory");
    let server = Running::start(&files.path().join("control.sqlite")).await;
    let mut person = Person::new(1, WEB_ORIGIN);
    let key = DeviceKey::generate();
    let credentials = person.register(&server, &key, "android", "Pixel 9").await;

    let mut straggler = connect(&server).await;
    authenticate(&mut straggler, &credentials).await;
    let nonce = challenge(&mut straggler).await;
    server
        .coordinator
        .revoke_device(credentials.device_id)
        .await
        .expect("revoke");

    send(
        &mut straggler,
        &ClientMessage::DeviceProof {
            signature: key.sign(&proof_for(&credentials, &nonce)),
        },
    )
    .await;
    expect_error(&mut straggler, ErrorCode::AuthenticationFailed).await;
    expect_closed(&mut straggler).await;
    server.stop().await;
}

async fn registration_options(
    server: &Running,
    bootstrap: &impl serde::Serialize,
    key: &DeviceKey,
) -> (StatusCode, header::HeaderMap, Value) {
    server
        .post(
            "/v1/identity/passkeys/registration/options",
            &json!({
                "bootstrapToken": bootstrap,
                "surface": "android",
                "device": { "publicKey": key.public_key(), "label": "Pixel 9" },
            }),
        )
        .await
}

#[tokio::test]
async fn a_registration_that_would_be_refused_leaves_its_bootstrap_unspent() {
    let files = tempfile::tempdir().expect("temporary directory");
    let server = Running::start(&files.path().join("control.sqlite")).await;
    let mut person = Person::new(1, WEB_ORIGIN);
    let key = DeviceKey::generate();
    let credentials = person.register(&server, &key, "android", "Pixel 9").await;
    server
        .coordinator
        .revoke_device(credentials.device_id)
        .await
        .expect("revoke");
    let bootstrap = server
        .coordinator
        .create_passkey_bootstrap(person.principal, std::time::SystemTime::now() + WAIT)
        .await
        .expect("bootstrap");

    // The signed-out key cannot register again.
    let (status, _, body) = registration_options(&server, &bootstrap, &key).await;
    assert_eq!(status, StatusCode::CONFLICT, "{body}");
    assert_eq!(
        body["message"],
        "this device key was signed out; create a new key to sign in again"
    );

    // Nor can a key that belongs to another person's sign-in.
    let mut other = Person::new(2, WEB_ORIGIN);
    let fresh = DeviceKey::generate();
    other.register(&server, &fresh, "android", "Tablet").await;
    let (status, _, body) = registration_options(&server, &bootstrap, &fresh).await;
    assert_eq!(status, StatusCode::CONFLICT, "{body}");
    assert_eq!(
        body["message"],
        "this device key is registered to another sign-in"
    );
    assert_eq!(
        server.rows("SELECT COUNT(*) FROM passkey_bootstraps"),
        1,
        "a refused registration leaves the one-use bootstrap unspent"
    );
    assert_eq!(
        server.rows("SELECT COUNT(*) FROM passkey_registration_ceremonies"),
        0
    );
    assert_eq!(server.rows("SELECT COUNT(*) FROM device_sign_ins"), 0);
    server.stop().await;
}

#[tokio::test]
async fn a_conflicting_key_is_refused_only_after_the_passkey_has_proved_the_person() {
    let files = tempfile::tempdir().expect("temporary directory");
    let server = Running::start(&files.path().join("control.sqlite")).await;
    let mut person = Person::new(1, WEB_ORIGIN);
    let signed_out = DeviceKey::generate();
    let credentials = person
        .register(&server, &signed_out, "android", "Pixel 9")
        .await;
    server
        .coordinator
        .revoke_device(credentials.device_id)
        .await
        .expect("revoke");
    let mut other = Person::new(2, WEB_ORIGIN);
    let taken = DeviceKey::generate();
    other.register(&server, &taken, "android", "Tablet").await;
    let devices = server.rows("SELECT COUNT(*) FROM devices");
    let counters = server.rows("SELECT SUM(authentication_counter) FROM passkeys");

    // Starting a sign-in needs no secret, so its answer cannot depend on the key.
    for key in [&signed_out, &taken, &DeviceKey::generate()] {
        let (status, body) = person
            .start_sign_in(&server, "android", Some((key, "Pixel 9")))
            .await;
        assert_eq!(status, StatusCode::OK, "{body}");
        assert!(body.get("message").is_none(), "{body}");
    }

    // After the assertion the refusal names its cause and changes nothing.
    for (key, message) in [
        (
            &signed_out,
            "this device key was signed out; create a new key to sign in again",
        ),
        (&taken, "this device key is registered to another sign-in"),
    ] {
        let (status, _, body) = person.try_sign_in(&server, key, "android", "Pixel 9").await;
        assert_eq!(status, StatusCode::CONFLICT, "{body}");
        assert_eq!(body["message"], message);
        assert_eq!(server.rows("SELECT COUNT(*) FROM devices"), devices);
        assert_eq!(server.rows("SELECT COUNT(*) FROM device_keys"), devices);
        assert_eq!(
            server.rows("SELECT SUM(authentication_counter) FROM passkeys"),
            counters,
            "a refused sign-in leaves the passkey as it was"
        );
    }
    person
        .sign_in(
            &server,
            &DeviceKey::generate(),
            "android",
            "Pixel 9 (new key)",
        )
        .await;
    server.stop().await;
}

#[tokio::test]
async fn the_device_limit_is_refused_after_the_passkey_and_frees_a_slot_on_sign_out() {
    let files = tempfile::tempdir().expect("temporary directory");
    let server = Running::start(&files.path().join("control.sqlite")).await;
    let mut person = Person::new(1, WEB_ORIGIN);
    person
        .register(&server, &DeviceKey::generate(), "android", "Device 0")
        .await;
    for index in 1..32 {
        let label = format!("Device {index}");
        person
            .sign_in(&server, &DeviceKey::generate(), "android", &label)
            .await;
    }
    assert_eq!(server.rows("SELECT COUNT(*) FROM device_keys"), 32);

    let extra = DeviceKey::generate();
    let (status, _, body) = person
        .try_sign_in(&server, &extra, "android", "One too many")
        .await;
    assert_eq!(status, StatusCode::CONFLICT, "{body}");
    assert!(
        body["message"]
            .as_str()
            .is_some_and(|message| message.contains("32 signed-in devices")),
        "{body}"
    );
    assert_eq!(server.rows("SELECT COUNT(*) FROM device_keys"), 32);

    server.execute(
        "UPDATE devices SET revoked = 1 WHERE device_id =
         (SELECT device_id FROM device_keys ORDER BY created_at_ms, device_id LIMIT 1)",
    );
    person
        .sign_in(&server, &extra, "android", "One too many")
        .await;
    assert_eq!(server.rows("SELECT COUNT(*) FROM device_keys"), 33);
    server.stop().await;
}

#[tokio::test]
async fn a_sign_in_with_a_bad_key_or_proof_leaves_no_residue() {
    let files = tempfile::tempdir().expect("temporary directory");
    let server = Running::start(&files.path().join("control.sqlite")).await;
    let mut person = Person::new(1, WEB_ORIGIN);
    let key = DeviceKey::generate();
    person.register(&server, &key, "android", "Pixel 9").await;
    let devices = server.rows("SELECT COUNT(*) FROM devices");

    // Malformed keys and labels are refused at options and start no ceremony.
    let valid = key.public_key();
    for device in [
        json!({ "publicKey": "AAAA", "label": "Pixel" }),
        json!({ "publicKey": format!("{valid}="), "label": "Pixel" }),
        json!({ "publicKey": valid, "label": "" }),
        json!({ "publicKey": valid, "label": "x".repeat(65) }),
        json!({ "publicKey": valid, "label": "Pixel", "extra": true }),
    ] {
        let (status, _, body) = server
            .post(
                "/v1/identity/passkeys/authentication/options",
                &json!({ "principalId": person.principal, "surface": "android", "device": device }),
            )
            .await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{device}: {body}");
    }
    assert_eq!(
        server.rows("SELECT COUNT(*) FROM passkey_authentication_ceremonies"),
        0
    );
    assert_eq!(server.rows("SELECT COUNT(*) FROM device_sign_ins"), 0);

    // A new key whose proof is made by another key, or missing, adds no device.
    let fresh = DeviceKey::generate();
    for proof in [Some(DeviceKey::generate()), None] {
        let (status, options) = person
            .start_sign_in(&server, "android", Some((&fresh, "Tablet")))
            .await;
        assert_eq!(status, StatusCode::OK, "{options}");
        let ceremony_id = options["ceremonyId"]
            .as_str()
            .expect("ceremony id")
            .to_owned();
        let credential = person.assert(&options);
        let mut request = json!({ "ceremonyId": ceremony_id, "credential": credential });
        if let Some(signer) = &proof {
            request["deviceProof"] =
                json!(signer.sign(&format!("renoa device registration v1\n{ceremony_id}")));
        }
        let (status, _, body) = server
            .post("/v1/identity/passkeys/authentication/verify", &request)
            .await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
        assert_eq!(server.rows("SELECT COUNT(*) FROM devices"), devices);
        assert_eq!(server.rows("SELECT COUNT(*) FROM device_keys"), devices);
        assert_eq!(server.rows("SELECT COUNT(*) FROM device_sign_ins"), 0);
    }

    // A browser ceremony carries no device proof.
    let (status, options) = person.start_sign_in(&server, "web", None).await;
    assert_eq!(status, StatusCode::OK);
    let credential = person.assert(&options);
    let (status, _, body) = server
        .post(
            "/v1/identity/passkeys/authentication/verify",
            &json!({
                "ceremonyId": options["ceremonyId"],
                "credential": credential,
                "deviceProof": key.sign("unneeded"),
            }),
        )
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert_eq!(
        server.rows("SELECT COUNT(*) FROM browser_connection_tickets"),
        0
    );
    server.stop().await;
}

#[tokio::test]
async fn an_expired_sign_in_adds_no_device_and_its_key_is_swept() {
    let files = tempfile::tempdir().expect("temporary directory");
    let server = Running::start(&files.path().join("control.sqlite")).await;
    let mut person = Person::new(1, WEB_ORIGIN);
    person
        .register(&server, &DeviceKey::generate(), "android", "Pixel 9")
        .await;
    let devices = server.rows("SELECT COUNT(*) FROM devices");

    let fresh = DeviceKey::generate();
    let (status, options) = person
        .start_sign_in(&server, "android", Some((&fresh, "Tablet")))
        .await;
    assert_eq!(status, StatusCode::OK, "{options}");
    assert_eq!(server.rows("SELECT COUNT(*) FROM device_sign_ins"), 1);
    server.execute(
        "UPDATE passkey_authentication_ceremonies SET expires_at_ms = 0;
         UPDATE device_sign_ins SET expires_at_ms = 0;",
    );

    // The ceremony ran out before it finished: no device, even with a valid proof.
    let ceremony_id = options["ceremonyId"]
        .as_str()
        .expect("ceremony id")
        .to_owned();
    let credential = person.assert(&options);
    let (status, _, body) = server
        .post(
            "/v1/identity/passkeys/authentication/verify",
            &json!({
                "ceremonyId": ceremony_id,
                "credential": credential,
                "deviceProof": fresh.sign(&format!("renoa device registration v1\n{ceremony_id}")),
            }),
        )
        .await;
    assert!(status.is_client_error(), "{status}: {body}");
    assert_eq!(server.rows("SELECT COUNT(*) FROM devices"), devices);
    assert_eq!(server.rows("SELECT COUNT(*) FROM device_keys"), devices);

    // The next ceremony sweeps the key that was stored beside the expired one.
    let (status, _) = person.start_sign_in(&server, "web", None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(server.rows("SELECT COUNT(*) FROM device_sign_ins"), 0);
    server.stop().await;
}

/// An app the test signed in, with the key that proves it.
struct App {
    credentials: DeviceCredentials,
    key: DeviceKey,
}

impl App {
    async fn connect(&self, server: &Running) -> Socket {
        connect_device(server, &self.credentials, &self.key).await
    }
}

/// A person who signed in a phone with a new passkey and a tablet with it.
async fn owner_with_two_apps(server: &Running) -> (Person, App, App) {
    let mut owner = Person::new(1, WEB_ORIGIN);
    let phone_key = DeviceKey::generate();
    let phone = owner.register(server, &phone_key, "android", "Phone").await;
    let tablet_key = DeviceKey::generate();
    let tablet = owner
        .sign_in(server, &tablet_key, "android", "Tablet")
        .await;
    let phone = App {
        credentials: phone,
        key: phone_key,
    };
    let tablet = App {
        credentials: tablet,
        key: tablet_key,
    };
    (owner, phone, tablet)
}

async fn strangers_app(server: &Running) -> App {
    let key = DeviceKey::generate();
    let credentials = Person::new(2, WEB_ORIGIN)
        .register(server, &key, "android", "Other")
        .await;
    App { credentials, key }
}

#[tokio::test]
async fn an_owner_lists_only_their_own_apps() {
    let files = tempfile::tempdir().expect("temporary directory");
    let server = Running::start(&files.path().join("control.sqlite")).await;
    let (_owner, phone, tablet) = owner_with_two_apps(&server).await;
    let stranger = strangers_app(&server).await;
    let mut phone_socket = phone.connect(&server).await;
    let mut stranger_socket = stranger.connect(&server).await;

    let devices = list_devices(&mut phone_socket, 1).await;
    assert_eq!(
        devices
            .iter()
            .map(|d| (d.device_id, d.label.as_str(), d.current))
            .collect::<Vec<_>>(),
        vec![
            (phone.credentials.device_id, "Phone", true),
            (tablet.credentials.device_id, "Tablet", false)
        ],
        "the owner's apps, oldest first, with the asking one marked"
    );
    assert!(devices[0].created_at_ms <= devices[1].created_at_ms);
    let others = list_devices(&mut stranger_socket, 2).await;
    assert_eq!(others.len(), 1, "another person's apps are never listed");
    assert_eq!(others[0].device_id, stranger.credentials.device_id);
    server.stop().await;
}

#[tokio::test]
async fn only_a_person_bound_connection_may_revoke_and_only_the_owners_apps() {
    let files = tempfile::tempdir().expect("temporary directory");
    let server = Running::start(&files.path().join("control.sqlite")).await;
    let (_owner, phone, _tablet) = owner_with_two_apps(&server).await;
    let stranger = strangers_app(&server).await;
    let link = enroll_link(&server, 1, "discord").await;
    let mut phone_socket = phone.connect(&server).await;
    let mut stranger_socket = stranger.connect(&server).await;

    // Another person's device, an unknown one and a device without a key are not found.
    for device_id in [
        stranger.credentials.device_id,
        DeviceId::from_uuid(Uuid::from_u128(99)),
        link.device_id,
    ] {
        send(
            &mut phone_socket,
            &ClientMessage::RevokeDevice {
                request_id: 2,
                device_id,
            },
        )
        .await;
        expect_error(&mut phone_socket, ErrorCode::NotFound).await;
    }
    assert_eq!(list_devices(&mut stranger_socket, 3).await.len(), 1);

    // A bearer-only link is refused both operations, so a chat surface cannot cut the owner off.
    let mut link_socket = connect(&server).await;
    authenticate(&mut link_socket, &link).await;
    assert!(matches!(
        receive(&mut link_socket).await,
        ServerMessage::Authenticated { .. }
    ));
    send(
        &mut link_socket,
        &ClientMessage::ListDevices { request_id: 4 },
    )
    .await;
    expect_error(&mut link_socket, ErrorCode::InvalidRole).await;
    send(
        &mut link_socket,
        &ClientMessage::RevokeDevice {
            request_id: 5,
            device_id: phone.credentials.device_id,
        },
    )
    .await;
    expect_error(&mut link_socket, ErrorCode::InvalidRole).await;
    assert_eq!(
        list_devices(&mut phone_socket, 6).await.len(),
        2,
        "nothing was revoked"
    );
    server.stop().await;
}

#[tokio::test]
async fn an_owner_revokes_one_app_from_another_app_and_from_a_browser() {
    let files = tempfile::tempdir().expect("temporary directory");
    let server = Running::start(&files.path().join("control.sqlite")).await;
    let (mut owner, phone, tablet) = owner_with_two_apps(&server).await;
    let mut phone_socket = phone.connect(&server).await;
    let mut tablet_socket = tablet.connect(&server).await;

    // The phone revokes the tablet: acknowledged, the tablet's socket ends, it cannot return.
    send(
        &mut phone_socket,
        &ClientMessage::RevokeDevice {
            request_id: 7,
            device_id: tablet.credentials.device_id,
        },
    )
    .await;
    assert!(matches!(
        receive(&mut phone_socket).await,
        ServerMessage::DeviceRevoked { request_id: 7, device_id }
            if device_id == tablet.credentials.device_id
    ));
    expect_closed(&mut tablet_socket).await;
    let mut returning = connect(&server).await;
    authenticate(&mut returning, &tablet.credentials).await;
    expect_error(&mut returning, ErrorCode::AuthenticationFailed).await;

    // A retry of the same revocation succeeds again and changes nothing.
    send(
        &mut phone_socket,
        &ClientMessage::RevokeDevice {
            request_id: 8,
            device_id: tablet.credentials.device_id,
        },
    )
    .await;
    assert!(matches!(
        receive(&mut phone_socket).await,
        ServerMessage::DeviceRevoked { request_id: 8, .. }
    ));
    let devices = list_devices(&mut phone_socket, 9).await;
    assert_eq!(devices.len(), 1);
    assert_eq!(devices[0].device_id, phone.credentials.device_id);

    // A browser the owner signed in with a passkey can revoke too.
    let ticket = owner.browser_ticket(&server).await;
    let mut browser = connect(&server).await;
    send(
        &mut browser,
        &serde_json::from_value(json!({
            "type": "authenticate_ticket", "version": JSON_WS_VERSION, "ticket": ticket,
        }))
        .expect("ticket frame"),
    )
    .await;
    assert!(matches!(
        receive(&mut browser).await,
        ServerMessage::Authenticated { .. }
    ));
    let devices = list_devices(&mut browser, 10).await;
    assert_eq!(devices.len(), 1);
    assert!(!devices[0].current, "a browser is no listed device");
    send(
        &mut browser,
        &ClientMessage::RevokeDevice {
            request_id: 11,
            device_id: phone.credentials.device_id,
        },
    )
    .await;
    assert!(matches!(
        receive(&mut browser).await,
        ServerMessage::DeviceRevoked { .. }
    ));
    expect_closed(&mut phone_socket).await;
    server.stop().await;
}

#[tokio::test]
async fn a_device_that_revokes_itself_hears_back_before_its_socket_closes() {
    let files = tempfile::tempdir().expect("temporary directory");
    let server = Running::start(&files.path().join("control.sqlite")).await;
    let mut person = Person::new(1, WEB_ORIGIN);
    let first = DeviceKey::generate();
    person.register(&server, &first, "android", "First").await;
    for attempt in 0..12 {
        let key = DeviceKey::generate();
        let credentials = person
            .sign_in(&server, &key, "android", &format!("Device {attempt}"))
            .await;
        let mut socket = connect_device(&server, &credentials, &key).await;
        send(
            &mut socket,
            &ClientMessage::RevokeDevice {
                request_id: 1,
                device_id: credentials.device_id,
            },
        )
        .await;
        assert!(
            matches!(
                receive(&mut socket).await,
                ServerMessage::DeviceRevoked { request_id: 1, .. }
            ),
            "attempt {attempt}: the acknowledgement arrives before the close"
        );
        expect_closed(&mut socket).await;
    }
    server.stop().await;
}

fn fingerprint(byte: u8) -> String {
    vec![format!("{byte:02X}"); 32].join(":")
}

fn android_origin(byte: u8) -> String {
    format!(
        "android:apk-key-hash:{}",
        URL_SAFE_NO_PAD.encode([byte; 32])
    )
}

fn apps(entries: &Value) -> AndroidApps {
    AndroidApps::from_json(&entries.to_string()).expect("android apps")
}

#[tokio::test]
async fn only_a_configured_android_origin_may_sign_an_app_in() {
    let files = tempfile::tempdir().expect("temporary directory");
    let server = Running::start_with(
        &files.path().join("control.sqlite"),
        apps(&json!({ "apps": [{
            "packageName": "live.renoa.app",
            "sha256CertFingerprints": [fingerprint(0x11), fingerprint(0x22)],
        }]})),
    )
    .await;

    // An app signed by an unconfigured certificate is refused and registers nothing.
    let mut stranger = Person::new(1, &android_origin(0x33));
    let key = DeviceKey::generate();
    let bootstrap = server
        .coordinator
        .create_passkey_bootstrap(stranger.principal, std::time::SystemTime::now() + WAIT)
        .await
        .expect("bootstrap");
    let (status, _, options) = server
        .post(
            "/v1/identity/passkeys/registration/options",
            &json!({
                "bootstrapToken": bootstrap,
                "surface": "android",
                "device": { "publicKey": key.public_key(), "label": "Pixel 9" },
            }),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{options}");
    let ceremony_id = options["ceremonyId"]
        .as_str()
        .expect("ceremony id")
        .to_owned();
    let challenge: CreationChallengeResponse =
        serde_json::from_value(options["options"].clone()).expect("creation options");
    let credential = stranger
        .passkey
        .perform_register(stranger.origin.clone(), challenge.public_key, 60_000)
        .expect("register passkey");
    let (status, _, body) = server
        .post(
            "/v1/identity/passkeys/registration/verify",
            &json!({
                "ceremonyId": ceremony_id,
                "credential": credential,
                "deviceProof": key.sign(&format!("renoa device registration v1\n{ceremony_id}")),
            }),
        )
        .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED, "{body}");
    assert_eq!(server.rows("SELECT COUNT(*) FROM passkeys"), 0);
    assert_eq!(server.rows("SELECT COUNT(*) FROM devices"), 0);

    // Each configured certificate's origin is accepted, for registration and for sign-in.
    let mut person = Person::new(1, &android_origin(0x11));
    let first = DeviceKey::generate();
    person.register(&server, &first, "android", "Pixel 9").await;
    let second = DeviceKey::generate();
    person
        .sign_in(&server, &second, "android", "Pixel 9 (debug)")
        .await;
    person.origin = Url::parse(&android_origin(0x22)).expect("origin");
    let third = DeviceKey::generate();
    person
        .sign_in(&server, &third, "android", "Pixel 9 (other build)")
        .await;
    assert_eq!(server.rows("SELECT COUNT(*) FROM device_keys"), 3);

    // The same passkey asserted at an unconfigured origin is refused.
    person.origin = Url::parse(&android_origin(0x33)).expect("origin");
    let mallory = DeviceKey::generate();
    let (status, options) = person
        .start_sign_in(&server, "android", Some((&mallory, "Mallory")))
        .await;
    assert_eq!(status, StatusCode::OK, "{options}");
    let ceremony_id = options["ceremonyId"]
        .as_str()
        .expect("ceremony id")
        .to_owned();
    let credential = person.assert(&options);
    let (status, _, body) = server
        .post(
            "/v1/identity/passkeys/authentication/verify",
            &json!({
                "ceremonyId": ceremony_id,
                "credential": credential,
                "deviceProof": mallory.sign(&format!("renoa device registration v1\n{ceremony_id}")),
            }),
        )
        .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED, "{body}");
    assert_eq!(server.rows("SELECT COUNT(*) FROM device_keys"), 3);
    server.stop().await;
}

#[tokio::test]
async fn asset_links_are_served_only_when_apps_are_configured() {
    let files = tempfile::tempdir().expect("temporary directory");
    let database = files.path().join("control.sqlite");
    let without = Running::start(&database).await;
    let missing = reqwest::get(format!("{}/.well-known/assetlinks.json", without.http))
        .await
        .expect("request");
    assert_eq!(missing.status(), StatusCode::NOT_FOUND);
    without.stop().await;

    let server = Running::start_with(
        &database,
        apps(&json!({ "apps": [{
            "packageName": "live.renoa.app",
            "sha256CertFingerprints": [fingerprint(0xab), fingerprint(0x0c)],
        }]})),
    )
    .await;
    let response = reqwest::get(format!("{}/.well-known/assetlinks.json", server.http))
        .await
        .expect("request");
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response
            .headers()
            .get(header::CONTENT_TYPE)
            .expect("content type"),
        "application/json"
    );
    assert_eq!(
        response.json::<Value>().await.expect("asset links"),
        json!([{
            "relation": ["delegate_permission/common.get_login_creds"],
            "target": {
                "namespace": "android_app",
                "package_name": "live.renoa.app",
                "sha256_cert_fingerprints": [fingerprint(0xab), fingerprint(0x0c)],
            },
        }])
    );
    server.stop().await;
}

#[test]
fn the_android_apps_file_names_what_is_wrong() {
    let valid = json!({ "apps": [{
        "packageName": "live.renoa.app",
        "sha256CertFingerprints": [fingerprint(1)],
    }]});
    assert!(AndroidApps::from_json(&valid.to_string()).is_ok());
    let lower = fingerprint(0xab).to_lowercase();
    assert!(
        AndroidApps::from_json(
            &json!({ "apps": [{ "packageName": "live.renoa.app", "sha256CertFingerprints": [lower] }]})
                .to_string()
        )
        .is_ok(),
        "keytool's colons with either case"
    );

    let cases = [
        (json!({ "apps": [] }), "needs 1 to 4 entries"),
        (
            json!({ "apps": [{ "packageName": "renoa", "sha256CertFingerprints": [fingerprint(1)] }]}),
            "not an Android package name",
        ),
        (
            json!({ "apps": [{ "packageName": "live.1renoa", "sha256CertFingerprints": [fingerprint(1)] }]}),
            "not an Android package name",
        ),
        (
            json!({ "apps": [{ "packageName": "live.renoa.app", "sha256CertFingerprints": [] }]}),
            "needs 1 to 4",
        ),
        (
            json!({ "apps": [{ "packageName": "live.renoa.app", "sha256CertFingerprints": ["AA:BB"] }]}),
            "32 colon-separated hex bytes",
        ),
        (
            json!({ "apps": [{ "packageName": "live.renoa.app", "sha256CertFingerprints": [fingerprint(1).replace(':', "")] }]}),
            "32 colon-separated hex bytes",
        ),
        (
            json!({ "apps": [{ "packageName": "live.renoa.app", "sha256CertFingerprints": [fingerprint(1), fingerprint(1)] }]}),
            "lists a certificate twice",
        ),
        (
            json!({ "apps": [
                { "packageName": "live.renoa.app", "sha256CertFingerprints": [fingerprint(1)] },
                { "packageName": "live.renoa.other", "sha256CertFingerprints": [fingerprint(1)] },
            ]}),
            "repeats a package name or certificate",
        ),
        (
            json!({ "apps": [{ "packageName": "live.renoa.app", "sha256CertFingerprints": [fingerprint(1)], "extra": 1 }]}),
            "unknown field",
        ),
    ];
    for (file, expected) in cases {
        let error = AndroidApps::from_json(&file.to_string()).expect_err("an invalid file");
        assert!(
            error.to_string().contains(expected),
            "{file} should fail with {expected:?}, not {error}"
        );
    }
}

#[test]
fn the_documented_example_file_loads() {
    let example = include_str!("../../../deploy/renoa-android-apps.example.json");
    let apps = AndroidApps::from_json(example).expect("the checked-in example must load");
    assert!(!apps.is_empty());
}

#[test]
fn the_documented_apps_file_lies_in_the_tree_the_coordinator_unit_owns() {
    // The unit runs as an account that cannot read root's /etc/renoa. A
    // coordinator that cannot read the file named by RENOA_ANDROID_APPS_FILE
    // refuses to start, so a path outside its own tree takes RCP down.
    let readme = include_str!("../../../deploy/README.md");
    let unit = include_str!("../../../deploy/renoa-coordinator.service");
    let documented = readme
        .lines()
        .find_map(|line| {
            line.trim_start_matches(['#', ' '])
                .strip_prefix("Environment=RENOA_ANDROID_APPS_FILE=")
        })
        .expect("the README sets RENOA_ANDROID_APPS_FILE");
    let owned = unit
        .lines()
        .find_map(|line| line.strip_prefix("ReadWritePaths="))
        .expect("the unit names the paths the coordinator owns");
    assert!(
        Path::new(documented).starts_with(owned),
        "{documented} is outside {owned}"
    );
}

#[tokio::test]
async fn a_key_bound_credential_is_refused_on_the_node_only_relay_routes() {
    // The refusal is the relay's rule that only a node identity may use it, and
    // a key can be registered only to a surface. If a route ever accepted
    // surfaces, a key-bound credential would need the proof there too.
    let files = tempfile::tempdir().expect("temporary directory");
    let server = Running::start(&files.path().join("control.sqlite")).await;
    let mut person = Person::new(1, WEB_ORIGIN);
    let key = DeviceKey::generate();
    let credentials = person.register(&server, &key, "android", "Pixel 9").await;

    let response = reqwest::Client::new()
        .post(format!(
            "{}{}",
            server.http,
            renoa_oauth_relay_protocol::OAUTH_RELAYS_PATH
        ))
        .header(
            renoa_oauth_relay_protocol::DEVICE_ID_HEADER,
            credentials.device_id.to_string(),
        )
        .bearer_auth(credentials.credential.expose())
        .json(&json!({}))
        .send()
        .await
        .expect("relay request");
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    server.stop().await;
}
