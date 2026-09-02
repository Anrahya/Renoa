use std::{
    fmt::Write as _,
    time::{Duration, SystemTime},
};

use futures_util::{SinkExt as _, StreamExt as _};
use renoa_control::{
    ClientMessage, Coordinator, DeviceCredentials, JSON_WS_VERSION, NodeId, PeerIdentity,
    ServerMessage,
};
use renoa_credential_relay_protocol::{
    AcknowledgeCredentialRelayRequest, CREDENTIAL_RELAY_VERSION, CreateCredentialRelayRequest,
    CredentialRelayForm, CredentialRelayId, CredentialRelayKind, CredentialRelayStatus,
    SubmitCredentialRelayRequest,
};
use renoa_oauth_relay_protocol::{
    AcknowledgeOAuthRelayRequest, CreateOAuthRelayRequest, CreateOAuthRelayResponse,
    DEVICE_ID_HEADER, OAUTH_RELAY_VERSION, OAuthRelayId, OAuthRelayStatus,
};
use renoa_protocol::{PrincipalId, SurfaceRef};
use sha2::{Digest as _, Sha256};
use tempfile::TempDir;
use tokio::net::TcpListener;
use tokio_tungstenite::{connect_async, tungstenite::Message};
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

const STATE: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";

#[tokio::test]
async fn a_browser_credential_is_stored_only_as_authenticated_ciphertext_until_host_ack() {
    let files = TempDir::new().expect("temporary credential relay directory");
    let database = files.path().join("control.sqlite");
    let coordinator = Coordinator::open_with_passkeys(&database, "localhost", "http://localhost")
        .expect("open coordinator");
    let enrollment = coordinator
        .create_enrollment(
            PeerIdentity::Node {
                node_id: NodeId::new(),
            },
            SystemTime::now() + Duration::from_mins(1),
        )
        .await
        .expect("create node enrollment");
    let server = TestServer::start(coordinator).await;
    let node = enroll(&server.websocket, enrollment).await;
    let client = reqwest::Client::new();
    let relay_id = CredentialRelayId::from_uuid(Uuid::from_u128(84));
    let capability = "ab".repeat(32);
    let request = CreateCredentialRelayRequest {
        version: CREDENTIAL_RELAY_VERSION,
        relay_id,
        credential_id: "exa.default".to_owned(),
        kind: CredentialRelayKind::ApiToken,
        capability_digest: hex_sha256(capability.as_bytes()),
    };
    let created = authorized(
        client.post(format!("{}/v1/credential-relays", server.http)),
        &node,
    )
    .json(&request)
    .send()
    .await
    .expect("create credential relay");
    assert_eq!(created.status(), reqwest::StatusCode::OK);

    assert_credential_form(&client, &server.http, relay_id).await;
    let encrypted = submit_credential_twice(&client, &server.http, relay_id, &capability).await;
    assert_encrypted_then_acknowledge(
        &client,
        &server.http,
        &node,
        &database,
        relay_id,
        &capability,
        &encrypted,
    )
    .await;
    server.stop().await;
}

async fn assert_credential_form(
    client: &reqwest::Client,
    origin: &str,
    relay_id: CredentialRelayId,
) {
    let form = client
        .get(format!("{origin}/v1/credential-relays/{relay_id}/form"))
        .send()
        .await
        .expect("load setup metadata");
    assert_secure(form.headers());
    let form: CredentialRelayForm = form.json().await.expect("decode setup metadata");
    assert_eq!(form.credential_id, "exa.default");
    assert_eq!(form.kind, CredentialRelayKind::ApiToken);
}

async fn submit_credential_twice(
    client: &reqwest::Client,
    origin: &str,
    relay_id: CredentialRelayId,
    capability: &str,
) -> (String, String) {
    let encrypted = ("00".repeat(12), "cd".repeat(17));
    let submission = SubmitCredentialRelayRequest {
        version: CREDENTIAL_RELAY_VERSION,
        capability: capability.to_owned(),
        nonce: encrypted.0.clone(),
        ciphertext: encrypted.1.clone(),
    };
    let unauthorized = client
        .post(format!("{origin}/v1/credential-relays/{relay_id}/submit"))
        .json(&SubmitCredentialRelayRequest {
            version: CREDENTIAL_RELAY_VERSION,
            capability: "ef".repeat(32),
            nonce: encrypted.0.clone(),
            ciphertext: encrypted.1.clone(),
        })
        .send()
        .await
        .expect("reject the wrong credential capability");
    assert_eq!(unauthorized.status(), reqwest::StatusCode::UNAUTHORIZED);
    for action in ["submit", "repeat"] {
        let response = client
            .post(format!("{origin}/v1/credential-relays/{relay_id}/submit"))
            .json(&submission)
            .send()
            .await
            .unwrap_or_else(|error| panic!("{action} encrypted credential: {error}"));
        assert_eq!(response.status(), reqwest::StatusCode::OK);
    }
    let conflict = client
        .post(format!("{origin}/v1/credential-relays/{relay_id}/submit"))
        .json(&SubmitCredentialRelayRequest {
            version: CREDENTIAL_RELAY_VERSION,
            capability: capability.to_owned(),
            nonce: "11".repeat(12),
            ciphertext: "23".repeat(17),
        })
        .send()
        .await
        .expect("reject conflicting credential content");
    assert_eq!(conflict.status(), reqwest::StatusCode::CONFLICT);
    encrypted
}

async fn assert_encrypted_then_acknowledge(
    client: &reqwest::Client,
    origin: &str,
    node: &DeviceCredentials,
    database: &std::path::Path,
    relay_id: CredentialRelayId,
    capability: &str,
    encrypted: &(String, String),
) {
    let status = authorized(
        client.get(format!("{origin}/v1/credential-relays/{relay_id}")),
        node,
    )
    .send()
    .await
    .expect("load encrypted credential status");
    let status: CredentialRelayStatus = status.json().await.expect("decode relay status");
    assert!(matches!(
        status,
        CredentialRelayStatus::Submitted { nonce, ciphertext, .. }
            if nonce == encrypted.0 && ciphertext == encrypted.1
    ));
    let stored = stored_credential_relay(database, relay_id);
    assert_eq!(stored.0, hex_sha256(capability.as_bytes()));
    assert_ne!(stored.0, capability);
    assert_eq!((stored.1, stored.2), encrypted.clone());

    let acknowledged = authorized(
        client.post(format!(
            "{origin}/v1/credential-relays/{relay_id}/acknowledge"
        )),
        node,
    )
    .json(&AcknowledgeCredentialRelayRequest {
        version: CREDENTIAL_RELAY_VERSION,
    })
    .send()
    .await
    .expect("acknowledge stored credential");
    assert_eq!(acknowledged.status(), reqwest::StatusCode::OK);
    let status = authorized(
        client.get(format!("{origin}/v1/credential-relays/{relay_id}")),
        node,
    )
    .send()
    .await
    .expect("load acknowledged status");
    let status: CredentialRelayStatus = status.json().await.expect("decode acknowledged status");
    assert!(matches!(status, CredentialRelayStatus::Acknowledged { .. }));
    assert_eq!(cleared_credential_relay(database, relay_id), (true, true));
}

fn stored_credential_relay(
    database: &std::path::Path,
    relay_id: CredentialRelayId,
) -> (String, String, String) {
    rusqlite::Connection::open(database)
        .expect("open coordinator database")
        .query_row(
            "SELECT capability_digest, nonce, ciphertext FROM credential_relays
             WHERE relay_id = ?1",
            [relay_id.to_string()],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .expect("inspect encrypted relay")
}

fn cleared_credential_relay(
    database: &std::path::Path,
    relay_id: CredentialRelayId,
) -> (bool, bool) {
    rusqlite::Connection::open(database)
        .expect("reopen coordinator database")
        .query_row(
            "SELECT nonce IS NULL, ciphertext IS NULL FROM credential_relays
             WHERE relay_id = ?1",
            [relay_id.to_string()],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .expect("inspect acknowledged relay")
}

#[tokio::test]
async fn a_node_callback_is_durable_idempotent_and_fail_closed() {
    let files = TempDir::new().expect("temporary control directory");
    let database = files.path().join("control.sqlite");
    let coordinator = Coordinator::open_with_passkeys(&database, "localhost", "http://localhost")
        .expect("open coordinator");
    let node_token = coordinator
        .create_enrollment(
            PeerIdentity::Node {
                node_id: NodeId::new(),
            },
            SystemTime::now() + Duration::from_mins(1),
        )
        .await
        .expect("create node enrollment");
    let surface_token = coordinator
        .create_enrollment(
            PeerIdentity::Surface {
                principal_id: PrincipalId::new(),
                surface: SurfaceRef::new("oauth-test"),
            },
            SystemTime::now() + Duration::from_mins(1),
        )
        .await
        .expect("create surface enrollment");
    let mut server = TestServer::start(coordinator).await;
    let node = enroll(&server.websocket, node_token).await;
    let surface = enroll(&server.websocket, surface_token).await;
    let client = reqwest::Client::new();
    let relay_id = OAuthRelayId::from_uuid(Uuid::from_u128(42));
    let request = CreateOAuthRelayRequest {
        version: OAUTH_RELAY_VERSION,
        relay_id,
        state_digest: hex_sha256(STATE.as_bytes()),
    };

    let surface_rejected = authorized(
        client
            .post(format!("{}/v1/oauth/relays", server.http))
            .json(&request),
        &surface,
    )
    .send()
    .await
    .expect("surface relay request");
    assert_eq!(surface_rejected.status(), reqwest::StatusCode::UNAUTHORIZED);

    let created = create(&client, &server.http, &node, &request).await;
    assert_eq!(created.relay_id, relay_id);
    assert_eq!(created.redirect_uri, "http://localhost/v1/oauth/callback");
    let repeated = create(&client, &server.http, &node, &request).await;
    assert_eq!(repeated, created);
    assert!(matches!(
        status(&client, &server.http, &node, relay_id).await,
        OAuthRelayStatus::Pending { .. }
    ));

    server = prove_callback_survives_restart(&client, server, &database, &node, relay_id).await;

    let acknowledged = authorized(
        client.post(format!(
            "{}/v1/oauth/relays/{relay_id}/acknowledge",
            server.http
        )),
        &node,
    )
    .json(&AcknowledgeOAuthRelayRequest {
        version: OAUTH_RELAY_VERSION,
    })
    .send()
    .await
    .expect("acknowledge callback");
    assert_eq!(acknowledged.status(), reqwest::StatusCode::OK);
    assert!(matches!(
        status(&client, &server.http, &node, relay_id).await,
        OAuthRelayStatus::Acknowledged { .. }
    ));

    let duplicate = client
        .get(format!(
            "{}/v1/oauth/callback?code=one-time-code&state={STATE}&iss=https%3A%2F%2Fissuer.example",
            server.http
        ))
        .send()
        .await
        .expect("repeat exact callback");
    assert_eq!(duplicate.status(), reqwest::StatusCode::OK);
    let conflict = client
        .get(format!(
            "{}/v1/oauth/callback?code=different-code&state={STATE}&iss=https%3A%2F%2Fissuer.example",
            server.http
        ))
        .send()
        .await
        .expect("send conflicting callback");
    assert_eq!(conflict.status(), reqwest::StatusCode::BAD_REQUEST);
    server.stop().await;
}

async fn prove_callback_survives_restart(
    client: &reqwest::Client,
    server: TestServer,
    database: &std::path::Path,
    node: &DeviceCredentials,
    relay_id: OAuthRelayId,
) -> TestServer {
    let callback = client
        .get(format!(
            "{}/v1/oauth/callback?code=one-time-code&state={STATE}&iss=https%3A%2F%2Fissuer.example",
            server.http
        ))
        .send()
        .await
        .expect("send provider callback");
    assert_eq!(callback.status(), reqwest::StatusCode::OK);
    assert_secure(callback.headers());
    let callback_body = callback.text().await.expect("read callback page");
    assert!(!callback_body.contains("one-time-code"));

    let OAuthRelayStatus::Authorized {
        authorization_code,
        issuer,
        ..
    } = status(client, &server.http, node, relay_id).await
    else {
        panic!("callback must become durable authorization")
    };
    assert_eq!(authorization_code, "one-time-code");
    assert_eq!(issuer.as_deref(), Some("https://issuer.example"));

    server.stop().await;
    let coordinator = Coordinator::open_with_passkeys(database, "localhost", "http://localhost")
        .expect("reopen coordinator");
    let server = TestServer::start(coordinator).await;
    let OAuthRelayStatus::Authorized {
        authorization_code, ..
    } = status(client, &server.http, node, relay_id).await
    else {
        panic!("callback must survive coordinator restart")
    };
    assert_eq!(authorization_code, "one-time-code");
    server
}

struct TestServer {
    http: String,
    websocket: String,
    shutdown: CancellationToken,
    task: tokio::task::JoinHandle<()>,
}

impl TestServer {
    async fn start(coordinator: Coordinator) -> Self {
        let listener = TcpListener::bind(("127.0.0.1", 0))
            .await
            .expect("bind coordinator");
        let address = listener.local_addr().expect("coordinator address");
        let shutdown = CancellationToken::new();
        let server_shutdown = shutdown.clone();
        let task = tokio::spawn(async move {
            coordinator
                .serve(listener, server_shutdown)
                .await
                .expect("serve coordinator");
        });
        Self {
            http: format!("http://{address}"),
            websocket: format!("ws://{address}/connect"),
            shutdown,
            task,
        }
    }

    async fn stop(self) {
        self.shutdown.cancel();
        self.task.await.expect("coordinator task joins");
    }
}

async fn enroll(endpoint: &str, token: renoa_control::EnrollmentToken) -> DeviceCredentials {
    let (mut socket, _) = connect_async(endpoint).await.expect("connect enrollment");
    socket
        .send(Message::Text(
            serde_json::to_string(&ClientMessage::Enroll {
                version: JSON_WS_VERSION,
                token,
            })
            .expect("encode enrollment")
            .into(),
        ))
        .await
        .expect("send enrollment");
    let message = socket
        .next()
        .await
        .expect("enrollment response")
        .expect("valid enrollment response");
    let Message::Text(text) = message else {
        panic!("enrollment response must be text")
    };
    let ServerMessage::Enrolled { credentials, .. } =
        serde_json::from_str(&text).expect("decode enrollment response")
    else {
        panic!("coordinator must issue credentials")
    };
    credentials
}

async fn create(
    client: &reqwest::Client,
    origin: &str,
    credentials: &DeviceCredentials,
    request: &CreateOAuthRelayRequest,
) -> CreateOAuthRelayResponse {
    let response = authorized(
        client.post(format!("{origin}/v1/oauth/relays")),
        credentials,
    )
    .json(request)
    .send()
    .await
    .expect("create relay");
    assert_eq!(response.status(), reqwest::StatusCode::OK);
    assert_secure(response.headers());
    response.json().await.expect("decode relay reservation")
}

async fn status(
    client: &reqwest::Client,
    origin: &str,
    credentials: &DeviceCredentials,
    relay_id: OAuthRelayId,
) -> OAuthRelayStatus {
    let response = authorized(
        client.get(format!("{origin}/v1/oauth/relays/{relay_id}")),
        credentials,
    )
    .send()
    .await
    .expect("read relay status");
    assert_eq!(response.status(), reqwest::StatusCode::OK);
    response.json().await.expect("decode relay status")
}

fn authorized(
    request: reqwest::RequestBuilder,
    credentials: &DeviceCredentials,
) -> reqwest::RequestBuilder {
    request
        .header(DEVICE_ID_HEADER, credentials.device_id.to_string())
        .bearer_auth(credentials.credential.expose())
}

fn assert_secure(headers: &reqwest::header::HeaderMap) {
    assert_eq!(
        headers
            .get(reqwest::header::CACHE_CONTROL)
            .and_then(|value| value.to_str().ok()),
        Some("no-store")
    );
    assert_eq!(
        headers
            .get(reqwest::header::REFERRER_POLICY)
            .and_then(|value| value.to_str().ok()),
        Some("no-referrer")
    );
}

fn hex_sha256(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .fold(String::with_capacity(64), |mut output, byte| {
            write!(output, "{byte:02x}").expect("write SHA-256 hex");
            output
        })
}
