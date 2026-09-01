use std::{path::Path, time::Duration};

use futures_util::{SinkExt, StreamExt};
use renoa_control::{
    ClientMessage, ConnectionTicket, Coordinator, JSON_WS_VERSION, NodeId, ServerMessage, TaskId,
    TaskSpec,
};
use renoa_protocol::{PrincipalId, TargetRef};
use reqwest::StatusCode;
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use tokio::{net::TcpListener, task::JoinHandle};
use tokio_tungstenite::{connect_async, tungstenite::Message};
use tokio_util::sync::CancellationToken;
use url::Url;
use uuid::Uuid;
use webauthn_authenticator_rs::{WebauthnAuthenticator, softpasskey::SoftPasskey};
use webauthn_rs::prelude::{
    CreationChallengeResponse, PublicKeyCredential, RegisterPublicKeyCredential,
    RequestChallengeResponse,
};

const PASSKEY_ORIGIN: &str = "http://localhost";
const PRINCIPAL_UUID: Uuid = Uuid::from_u128(1);
const TASK_UUID: Uuid = Uuid::from_u128(2);

struct RunningCoordinator {
    http: String,
    websocket: String,
    shutdown: CancellationToken,
    task: JoinHandle<Result<(), renoa_control::ControlError>>,
}

impl RunningCoordinator {
    async fn start(database: &Path) -> Self {
        let coordinator = Coordinator::open_with_passkeys(database, "localhost", PASSKEY_ORIGIN)
            .expect("open passkey coordinator");
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind coordinator");
        let address = listener.local_addr().expect("coordinator address");
        let shutdown = CancellationToken::new();
        let task = tokio::spawn({
            let shutdown = shutdown.clone();
            async move { coordinator.serve(listener, shutdown).await }
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
        self.task
            .await
            .expect("coordinator task panicked")
            .expect("coordinator stopped cleanly");
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct RegistrationOptionsRequest<'a> {
    bootstrap_token: &'a renoa_control::PasskeyBootstrapToken,
    surface: &'a str,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct AuthenticationOptionsRequest<'a> {
    principal_id: PrincipalId,
    surface: &'a str,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct VerifyRequest<'a, T> {
    ceremony_id: Uuid,
    credential: &'a T,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct OptionsResponse<T> {
    ceremony_id: Uuid,
    options: T,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct TicketResponse {
    connection_ticket: ConnectionTicket,
    expires_at_ms: i64,
}

#[tokio::test]
async fn a_passkey_survives_restart_and_issues_single_use_surface_tickets() {
    let files = tempfile::tempdir().expect("temporary directory");
    let database = files.path().join("control.sqlite");
    let principal_id = PrincipalId::from_uuid(PRINCIPAL_UUID);
    let coordinator = Coordinator::open(&database).expect("open coordinator database");
    coordinator
        .create_task(TaskSpec {
            task_id: TaskId::from_uuid(TASK_UUID),
            principal_id,
            node_id: NodeId::from_uuid(Uuid::from_u128(3)),
            target: TargetRef::new("workspace:passkey-test"),
        })
        .await
        .expect("create task");
    let bootstrap = coordinator
        .create_passkey_bootstrap(
            principal_id,
            std::time::SystemTime::now() + Duration::from_mins(5),
        )
        .await
        .expect("create passkey bootstrap");
    drop(coordinator);

    let http = reqwest::Client::new();
    let origin = Url::parse(PASSKEY_ORIGIN).expect("passkey origin");
    let mut authenticator = WebauthnAuthenticator::new(SoftPasskey::new(true));
    let (second, registration_ticket) =
        register_after_restart(&database, &bootstrap, &http, &origin, &mut authenticator).await;
    assert_ticket_is_digest_only(&database, &registration_ticket.connection_ticket);
    assert_single_use_ticket(&second.websocket, registration_ticket.connection_ticket).await;
    reject_stale_overlapping_counter(&second, principal_id, &http, &origin, &mut authenticator)
        .await;

    let authentication =
        begin_authentication(&http, &second.http, principal_id, "phone_test").await;
    let authenticated = authenticator
        .do_authentication(origin.clone(), authentication.options)
        .expect("soft passkey authentication");
    second.stop().await;

    let third = RunningCoordinator::start(&database).await;
    let authentication_ticket = finish_authentication(
        &http,
        &third.http,
        authentication.ceremony_id,
        &authenticated,
    )
    .await;
    Box::pin(assert_single_concurrent_claim(
        &third.websocket,
        authentication_ticket.connection_ticket,
    ))
    .await;
    assert_expired_ticket_is_rejected(
        &database,
        &third,
        principal_id,
        &http,
        &origin,
        &mut authenticator,
    )
    .await;
    third.stop().await;
}

async fn register_after_restart(
    database: &Path,
    bootstrap: &renoa_control::PasskeyBootstrapToken,
    http: &reqwest::Client,
    origin: &Url,
    authenticator: &mut WebauthnAuthenticator<SoftPasskey>,
) -> (RunningCoordinator, TicketResponse) {
    let first = RunningCoordinator::start(database).await;
    let invalid_surface = http
        .post(format!(
            "{}/v1/identity/passkeys/registration/options",
            first.http
        ))
        .json(&RegistrationOptionsRequest {
            bootstrap_token: bootstrap,
            surface: "web/../../node",
        })
        .send()
        .await
        .expect("reject invalid surface");
    assert_eq!(invalid_surface.status(), StatusCode::BAD_REQUEST);
    let registration: OptionsResponse<CreationChallengeResponse> = post_json(
        http,
        &first.http,
        "/v1/identity/passkeys/registration/options",
        &RegistrationOptionsRequest {
            bootstrap_token: bootstrap,
            surface: "web_test",
        },
    )
    .await;
    let reused_bootstrap = http
        .post(format!(
            "{}/v1/identity/passkeys/registration/options",
            first.http
        ))
        .json(&RegistrationOptionsRequest {
            bootstrap_token: bootstrap,
            surface: "web_test",
        })
        .send()
        .await
        .expect("retry registration options");
    assert_eq!(reused_bootstrap.status(), StatusCode::UNAUTHORIZED);

    let registered = authenticator
        .do_registration(origin.clone(), registration.options)
        .expect("soft passkey registration");
    first.stop().await;

    let second = RunningCoordinator::start(database).await;
    let registration_ticket: TicketResponse = post_json(
        http,
        &second.http,
        "/v1/identity/passkeys/registration/verify",
        &VerifyRequest::<RegisterPublicKeyCredential> {
            ceremony_id: registration.ceremony_id,
            credential: &registered,
        },
    )
    .await;
    assert!(registration_ticket.expires_at_ms > 0);
    let replayed_registration = http
        .post(format!(
            "{}/v1/identity/passkeys/registration/verify",
            second.http
        ))
        .json(&VerifyRequest::<RegisterPublicKeyCredential> {
            ceremony_id: registration.ceremony_id,
            credential: &registered,
        })
        .send()
        .await
        .expect("replay registration verification");
    assert_eq!(replayed_registration.status(), StatusCode::UNAUTHORIZED);
    (second, registration_ticket)
}

fn assert_ticket_is_digest_only(database: &Path, ticket: &ConnectionTicket) {
    let stored_ticket = rusqlite::Connection::open(database)
        .expect("inspect stored ticket")
        .query_row(
            "SELECT ticket_hash FROM browser_connection_tickets",
            [],
            |row| row.get::<_, Vec<u8>>(0),
        )
        .expect("load stored ticket hash");
    assert_eq!(stored_ticket.len(), 32);
    assert_ne!(stored_ticket, ticket.expose().as_bytes());
}

async fn assert_single_use_ticket(endpoint: &str, ticket: ConnectionTicket) {
    assert!(authenticate_ticket_and_list(endpoint, ticket.clone()).await);
    assert!(!authenticate_ticket_and_list(endpoint, ticket).await);
}

async fn reject_stale_overlapping_counter(
    coordinator: &RunningCoordinator,
    principal_id: PrincipalId,
    http: &reqwest::Client,
    origin: &Url,
    authenticator: &mut WebauthnAuthenticator<SoftPasskey>,
) {
    let counter_a = begin_authentication(http, &coordinator.http, principal_id, "counter_a").await;
    let counter_b = begin_authentication(http, &coordinator.http, principal_id, "counter_b").await;
    let credential_a = authenticator
        .do_authentication(origin.clone(), counter_a.options)
        .expect("first overlapping authentication");
    let credential_b = authenticator
        .do_authentication(origin.clone(), counter_b.options)
        .expect("second overlapping authentication");
    let newest_ticket = finish_authentication(
        http,
        &coordinator.http,
        counter_b.ceremony_id,
        &credential_b,
    )
    .await;
    assert!(
        authenticate_ticket_and_list(&coordinator.websocket, newest_ticket.connection_ticket).await
    );
    let stale_counter = http
        .post(format!(
            "{}/v1/identity/passkeys/authentication/verify",
            coordinator.http
        ))
        .json(&VerifyRequest::<PublicKeyCredential> {
            ceremony_id: counter_a.ceremony_id,
            credential: &credential_a,
        })
        .send()
        .await
        .expect("reject stale authentication counter");
    assert_eq!(stale_counter.status(), StatusCode::UNAUTHORIZED);
}

async fn begin_authentication(
    http: &reqwest::Client,
    base: &str,
    principal_id: PrincipalId,
    surface: &str,
) -> OptionsResponse<RequestChallengeResponse> {
    post_json(
        http,
        base,
        "/v1/identity/passkeys/authentication/options",
        &AuthenticationOptionsRequest {
            principal_id,
            surface,
        },
    )
    .await
}

async fn finish_authentication(
    http: &reqwest::Client,
    base: &str,
    ceremony_id: Uuid,
    credential: &PublicKeyCredential,
) -> TicketResponse {
    post_json(
        http,
        base,
        "/v1/identity/passkeys/authentication/verify",
        &VerifyRequest::<PublicKeyCredential> {
            ceremony_id,
            credential,
        },
    )
    .await
}

async fn assert_single_concurrent_claim(endpoint: &str, ticket: ConnectionTicket) {
    let (left, right) = tokio::join!(
        authenticate_ticket_and_list(endpoint, ticket.clone()),
        authenticate_ticket_and_list(endpoint, ticket)
    );
    assert_ne!(left, right, "exactly one concurrent ticket claim must win");
}

async fn assert_expired_ticket_is_rejected(
    database: &Path,
    coordinator: &RunningCoordinator,
    principal_id: PrincipalId,
    http: &reqwest::Client,
    origin: &Url,
    authenticator: &mut WebauthnAuthenticator<SoftPasskey>,
) {
    let authentication =
        begin_authentication(http, &coordinator.http, principal_id, "expired_test").await;
    let credential = authenticator
        .do_authentication(origin.clone(), authentication.options)
        .expect("soft passkey authentication for expiry");
    let expired_ticket = finish_authentication(
        http,
        &coordinator.http,
        authentication.ceremony_id,
        &credential,
    )
    .await;
    assert_ticket_is_digest_only(database, &expired_ticket.connection_ticket);

    let connection = rusqlite::Connection::open(database).expect("inspect identity database");
    connection
        .execute(
            "UPDATE browser_connection_tickets SET expires_at_ms = 0",
            [],
        )
        .expect("expire connection ticket");
    assert!(
        !authenticate_ticket_and_list(&coordinator.websocket, expired_ticket.connection_ticket)
            .await
    );
    let devices = connection
        .query_row("SELECT COUNT(*) FROM devices", [], |row| {
            row.get::<_, i64>(0)
        })
        .expect("count devices");
    assert_eq!(
        devices, 0,
        "a browser ticket must not invent a durable device"
    );
}

async fn post_json<T: DeserializeOwned>(
    client: &reqwest::Client,
    base: &str,
    path: &str,
    body: &impl Serialize,
) -> T {
    let response = client
        .post(format!("{base}{path}"))
        .json(body)
        .send()
        .await
        .expect("identity request");
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response
            .headers()
            .get(reqwest::header::CACHE_CONTROL)
            .expect("cache-control header"),
        "no-store"
    );
    response.json().await.expect("identity response JSON")
}

async fn authenticate_ticket_and_list(endpoint: &str, ticket: ConnectionTicket) -> bool {
    let (mut socket, _) = connect_async(endpoint)
        .await
        .expect("connect ticket surface");
    send(
        &mut socket,
        &ClientMessage::AuthenticateTicket {
            version: JSON_WS_VERSION,
            ticket,
        },
    )
    .await;
    match receive(&mut socket).await {
        ServerMessage::Authenticated { version } => {
            assert_eq!(version, JSON_WS_VERSION);
            send(&mut socket, &ClientMessage::ListTasks { request_id: 1 }).await;
            let ServerMessage::TaskList { tasks, .. } = receive(&mut socket).await else {
                panic!("authenticated ticket did not receive a task list");
            };
            assert_eq!(tasks.len(), 1);
            assert_eq!(tasks[0].task_id, TaskId::from_uuid(TASK_UUID));
            true
        }
        ServerMessage::Error { code, .. } => {
            assert_eq!(code, renoa_control::ErrorCode::AuthenticationFailed);
            false
        }
        message => panic!("unexpected ticket authentication response: {message:?}"),
    }
}

async fn send(
    socket: &mut tokio_tungstenite::WebSocketStream<
        tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
    >,
    message: &ClientMessage,
) {
    socket
        .send(Message::Text(
            serde_json::to_string(message)
                .expect("serialize client message")
                .into(),
        ))
        .await
        .expect("send client message");
}

async fn receive(
    socket: &mut tokio_tungstenite::WebSocketStream<
        tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
    >,
) -> ServerMessage {
    let message = socket
        .next()
        .await
        .expect("server closed socket")
        .expect("receive server message");
    let Message::Text(json) = message else {
        panic!("expected text message");
    };
    serde_json::from_str(&json).expect("parse server message")
}
