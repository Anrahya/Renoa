use std::{
    fs,
    net::SocketAddr,
    path::PathBuf,
    time::{Duration, SystemTime},
};

use futures_util::{SinkExt as _, StreamExt as _};
use renoa_control::{
    ClientMessage, Coordinator, DeviceCredentials, JSON_WS_VERSION, NodeId, PeerIdentity,
    ServerMessage,
};
use renoa_oauth_relay_protocol::{DEVICE_ID_HEADER, OAuthRelayId, OAuthRelayStatus};
use tempfile::{TempDir, tempdir};
use tokio::net::TcpListener;
use tokio_tungstenite::{connect_async, tungstenite::Message};
use tokio_util::sync::CancellationToken;

use super::super::{
    McpAuthorizationResolver,
    secret_store::OAuthSecretBundle,
    store::{OAuthCallbackIdentity, OAuthPhase},
};
use super::support::{CONNECTION, ENDPOINT, write_adapter};
use crate::mcp::{
    McpCatalogStore, McpConnectionAuth, McpCredentialResolver, McpHostError,
    McpOAuthAuthorizationRequest, McpOAuthError, McpOAuthRegistration, McpRequestHeaders,
};

const REJECTED_CONNECTION: &str = "oauth-rejected-fixture";

#[tokio::test]
async fn a_remote_callback_resumes_and_is_acknowledged_only_after_local_persistence() {
    let fixture = RemoteFixture::start().await;
    prove_interrupted_success(&fixture).await;
    prove_rejection(&fixture).await;
    fixture.stop().await;
}

struct RemoteFixture {
    _directory: TempDir,
    address: SocketAddr,
    public_origin: String,
    credentials: DeviceCredentials,
    control_database: PathBuf,
    host_database: PathBuf,
    store: McpCatalogStore,
    auth: McpConnectionAuth,
    resolver: McpAuthorizationResolver,
    server_shutdown: CancellationToken,
    server: tokio::task::JoinHandle<()>,
}

impl RemoteFixture {
    async fn start() -> Self {
        let directory = tempdir().expect("temporary remote OAuth fixture");
        let control_database = directory.path().join("control.sqlite3");
        let listener = TcpListener::bind(("127.0.0.1", 0))
            .await
            .expect("bind coordinator");
        let address = listener.local_addr().expect("coordinator address");
        let public_origin = format!("http://localhost:{}", address.port());
        let coordinator =
            Coordinator::open_with_passkeys(&control_database, "localhost", &public_origin)
                .expect("open coordinator");
        let enrollment = coordinator
            .create_enrollment(
                PeerIdentity::Node {
                    node_id: NodeId::new(),
                },
                SystemTime::now() + Duration::from_mins(1),
            )
            .await
            .expect("create OAuth Host enrollment");
        let server_shutdown = CancellationToken::new();
        let shutdown = server_shutdown.clone();
        let server = tokio::spawn(async move {
            coordinator
                .serve(listener, shutdown)
                .await
                .expect("serve coordinator");
        });
        let credentials = enroll(&format!("ws://{address}/connect"), enrollment).await;
        let credential_file = directory.path().join("relay-device.json");
        fs::write(
            &credential_file,
            serde_json::to_vec(&credentials).expect("encode relay credentials"),
        )
        .expect("write relay credentials");
        make_private(&credential_file);

        let host_database = directory.path().join("host.sqlite3");
        let store =
            McpCatalogStore::initialize(host_database.clone()).expect("initialize Host catalog");
        let auth = McpConnectionAuth::oauth(CONNECTION, ENDPOINT, McpOAuthRegistration::dynamic())
            .expect("OAuth connection reference");
        store
            .register_connection(
                "oauth-integration",
                CONNECTION,
                ENDPOINT,
                &McpRequestHeaders::default(),
                &auth,
            )
            .expect("register OAuth connection");
        let actions = directory.path().join("adapter-actions.txt");
        let adapter = write_adapter(directory.path(), &actions);
        let resolver = McpAuthorizationResolver::with_remote_oauth(
            &store,
            Some(adapter),
            McpCredentialResolver::default(),
            &public_origin,
            &credential_file,
        )
        .expect("configure remote OAuth callback");

        Self {
            _directory: directory,
            address,
            public_origin,
            credentials,
            control_database,
            host_database,
            store,
            auth,
            resolver,
            server_shutdown,
            server,
        }
    }

    async fn stop(self) {
        self.server_shutdown.cancel();
        self.server.await.expect("coordinator task joins");
    }
}

async fn prove_interrupted_success(fixture: &RemoteFixture) {
    let cancellation = CancellationToken::new();
    let interrupted = {
        let resolver = fixture.resolver.clone();
        let auth = fixture.auth.clone();
        let cancellation = cancellation.clone();
        tokio::spawn(async move {
            resolver
                .authorize(request(CONNECTION, "tool-call", &auth), cancellation)
                .await
        })
    };
    wait_for_phase(&fixture.resolver, CONNECTION, OAuthPhase::AwaitingCallback).await;
    let relay_id = load_relay_id(&fixture.resolver, CONNECTION).await;
    let bundle = load_bundle(&fixture.resolver, &fixture.auth).await;
    let state = bundle.adapter_state["csrf_state"]
        .as_str()
        .expect("saved OAuth state")
        .to_owned();
    assert_eq!(
        bundle.adapter_state["redirect_uri"].as_str(),
        Some(format!("{}/v1/oauth/callback", fixture.public_origin).as_str())
    );
    cancellation.cancel();
    assert!(
        interrupted
            .await
            .expect("interrupted authorization joins")
            .is_err()
    );

    let callback = reqwest::Client::new()
        .get(format!(
            "http://{}/v1/oauth/callback?code=code-one&state={state}",
            fixture.address
        ))
        .send()
        .await
        .expect("send provider callback");
    assert_eq!(callback.status(), reqwest::StatusCode::OK);

    let authorization = fixture
        .resolver
        .authorize(
            request(CONNECTION, "tool-call", &fixture.auth),
            CancellationToken::new(),
        )
        .await
        .expect("resume remote authorization");
    assert_eq!(authorization.secret(), "access-one");
    let status = relay_status(&fixture.public_origin, &fixture.credentials, relay_id).await;
    assert!(matches!(status, OAuthRelayStatus::Acknowledged { .. }));
    assert!(
        !fs::read(&fixture.host_database)
            .expect("read Host database")
            .windows("code-one".len())
            .any(|window| window == b"code-one")
    );
    assert!(
        !fs::read(&fixture.control_database)
            .expect("read control database")
            .windows("code-one".len())
            .any(|window| window == b"code-one")
    );
}

async fn prove_rejection(fixture: &RemoteFixture) {
    let rejected_auth = McpConnectionAuth::oauth(
        REJECTED_CONNECTION,
        ENDPOINT,
        McpOAuthRegistration::dynamic(),
    )
    .expect("rejected OAuth connection reference");
    fixture
        .store
        .register_connection(
            "oauth-rejected-integration",
            REJECTED_CONNECTION,
            ENDPOINT,
            &McpRequestHeaders::default(),
            &rejected_auth,
        )
        .expect("register rejected OAuth connection");
    let rejected = {
        let resolver = fixture.resolver.clone();
        let auth = rejected_auth.clone();
        tokio::spawn(async move {
            resolver
                .authorize(
                    request(REJECTED_CONNECTION, "rejected-call", &auth),
                    CancellationToken::new(),
                )
                .await
        })
    };
    wait_for_phase(
        &fixture.resolver,
        REJECTED_CONNECTION,
        OAuthPhase::AwaitingCallback,
    )
    .await;
    let rejected_relay_id = load_relay_id(&fixture.resolver, REJECTED_CONNECTION).await;
    let rejected_bundle = load_bundle(&fixture.resolver, &rejected_auth).await;
    let rejected_state = rejected_bundle.adapter_state["csrf_state"]
        .as_str()
        .expect("saved rejected OAuth state");
    let callback = reqwest::Client::new()
        .get(format!(
            "http://{}/v1/oauth/callback?error=access_denied&state={rejected_state}",
            fixture.address
        ))
        .send()
        .await
        .expect("send provider rejection");
    assert_eq!(callback.status(), reqwest::StatusCode::OK);
    let result = rejected.await.expect("rejected authorization joins");
    assert!(matches!(
        result,
        Err(McpHostError::OAuth(McpOAuthError::CallbackRejected(error)))
            if error == "access_denied"
    ));
    let status = relay_status(
        &fixture.public_origin,
        &fixture.credentials,
        rejected_relay_id,
    )
    .await;
    assert!(matches!(status, OAuthRelayStatus::Acknowledged { .. }));
    assert!(
        fixture
            .resolver
            .oauth
            .flows
            .load(REJECTED_CONNECTION)
            .await
            .expect("load rejected flow")
            .is_none()
    );
}

fn request<'a>(
    connection_id: &'a str,
    operation: &'a str,
    auth: &'a McpConnectionAuth,
) -> McpOAuthAuthorizationRequest<'a> {
    McpOAuthAuthorizationRequest {
        connection_id,
        display_name: None,
        endpoint: ENDPOINT,
        reference: auth,
        operation_id: operation,
        restart: false,
        requested_scope: None,
        updates: None,
    }
}

async fn wait_for_phase(
    resolver: &McpAuthorizationResolver,
    connection_id: &str,
    expected: OAuthPhase,
) {
    for _ in 0..300 {
        if resolver
            .oauth
            .flows
            .load(connection_id)
            .await
            .expect("load OAuth flow")
            .is_some_and(|flow| flow.phase == expected)
        {
            return;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    panic!("OAuth flow did not reach {expected:?}");
}

async fn load_bundle(
    resolver: &McpAuthorizationResolver,
    auth: &McpConnectionAuth,
) -> OAuthSecretBundle {
    resolver
        .oauth
        .secrets
        .load(
            auth.oauth_credential_id().expect("OAuth credential id"),
            CancellationToken::new(),
        )
        .await
        .expect("load private OAuth state")
        .expect("private OAuth state exists")
}

async fn relay_status(
    origin: &str,
    credentials: &DeviceCredentials,
    relay_id: OAuthRelayId,
) -> OAuthRelayStatus {
    let response = reqwest::Client::new()
        .get(format!("{origin}/v1/oauth/relays/{relay_id}"))
        .header(DEVICE_ID_HEADER, credentials.device_id.to_string())
        .bearer_auth(credentials.credential.expose())
        .send()
        .await
        .expect("load relay status");
    let body = response.bytes().await.expect("read relay status");
    serde_json::from_slice(&body).expect("decode relay status")
}

async fn load_relay_id(resolver: &McpAuthorizationResolver, connection_id: &str) -> OAuthRelayId {
    let flow = resolver
        .oauth
        .flows
        .load(connection_id)
        .await
        .expect("load awaiting callback flow")
        .expect("awaiting callback flow exists");
    let Some(OAuthCallbackIdentity::Relay(relay_id)) = flow.callback else {
        panic!("remote OAuth flow must retain its relay identity")
    };
    relay_id
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

#[cfg(unix)]
fn make_private(path: &std::path::Path) {
    use std::os::unix::fs::PermissionsExt as _;
    fs::set_permissions(path, fs::Permissions::from_mode(0o600)).expect("protect relay credential");
}

#[cfg(not(unix))]
fn make_private(_path: &std::path::Path) {}
