//! The owner's `/v1/host` calls, relayed by a real coordinator to a node that
//! the tests play by hand, so each frame the node sees and sends is explicit.

use std::{
    path::Path,
    time::{Duration, SystemTime},
};

use futures_util::{SinkExt, StreamExt};
use renoa_control::{BrowserSessions, Coordinator, PeerIdentity};
use renoa_protocol::{PrincipalId, SurfaceRef};
use renoa_rcp_wire::{
    ClientMessage, DeviceCredentials, ErrorCode, HostMethod, JSON_WS_VERSION, NodeId, ServerMessage,
};
use reqwest::{Client, Method, RequestBuilder, StatusCode, header::HeaderMap};
use serde_json::{Value, json};
use tokio::{net::TcpListener, task::JoinHandle, time::timeout};
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream, connect_async, tungstenite::Message};
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

const ORIGIN: &str = "http://localhost";
const QUIET: Duration = Duration::from_millis(150);

type Socket = WebSocketStream<MaybeTlsStream<tokio::net::TcpStream>>;

async fn send(socket: &mut Socket, message: &ClientMessage) {
    let json = serde_json::to_string(message).expect("serialize client message");
    socket
        .send(Message::Text(json.into()))
        .await
        .expect("send client message");
}

async fn receive(socket: &mut Socket) -> ServerMessage {
    loop {
        let message = timeout(Duration::from_secs(5), socket.next())
            .await
            .expect("a server message arrives")
            .expect("the socket stays open")
            .expect("valid websocket message");
        match message {
            Message::Text(json) => return serde_json::from_str(&json).expect("server message"),
            Message::Ping(_) | Message::Pong(_) => {}
            other => panic!("unexpected websocket message: {other:?}"),
        }
    }
}

/// Whether the node receives nothing within a quiet moment.
async fn hears_nothing(socket: &mut Socket) -> bool {
    timeout(QUIET, socket.next()).await.is_err()
}

#[derive(Clone)]
struct Caller {
    client: Client,
    http: String,
    cookie: Option<String>,
}

impl Caller {
    fn with_cookie(&self, builder: RequestBuilder) -> RequestBuilder {
        match &self.cookie {
            Some(cookie) => builder.header("cookie", cookie),
            None => builder,
        }
    }

    fn get(&self, path: &str) -> RequestBuilder {
        self.with_cookie(self.client.get(format!("{}{path}", self.http)))
    }

    /// A write as the Control Room sends it: the page's origin and JSON.
    fn write(&self, method: Method, path: &str, body: &Value) -> RequestBuilder {
        self.with_cookie(
            self.client
                .request(method, format!("{}{path}", self.http))
                .header("origin", ORIGIN)
                .json(body),
        )
    }

    fn anonymous(&self) -> Self {
        Self {
            cookie: None,
            ..self.clone()
        }
    }
}

struct Answer {
    status: StatusCode,
    headers: HeaderMap,
    text: String,
}

impl Answer {
    fn json(&self) -> Value {
        serde_json::from_str(&self.text).expect("a JSON body")
    }

    fn code(&self) -> String {
        self.json()["code"]
            .as_str()
            .expect("an error code")
            .to_owned()
    }
}

/// What the owner reads in a refusal names none of the machinery behind it.
fn assert_plain(answer: &Answer) {
    let message = answer.json()["message"]
        .as_str()
        .expect("a message")
        .to_lowercase();
    for word in ["coordinator", "node", "relay", "socket"] {
        assert!(!message.contains(word), "{word:?} in {message:?}");
    }
}

async fn answer_of(request: RequestBuilder) -> Answer {
    let response = request.send().await.expect("HTTP answer");
    Answer {
        status: response.status(),
        headers: response.headers().clone(),
        text: response.text().await.expect("body"),
    }
}

/// Says the node serves Host control and waits until the coordinator has
/// acted on it. A node connection handles its messages in order, so the
/// refusal of a surface-only message proves the advertisement was processed.
async fn advertise(node: &mut Socket) {
    send(node, &ClientMessage::ServeHostControl).await;
    send(node, &ClientMessage::ListTasks { request_id: 1 }).await;
    assert!(matches!(
        receive(node).await,
        ServerMessage::Error {
            code: ErrorCode::InvalidRole,
            ..
        }
    ));
}

struct Fixture {
    files: tempfile::TempDir,
    coordinator: Coordinator,
    ws: String,
    stop: CancellationToken,
    task: JoinHandle<Result<(), renoa_control::ControlError>>,
    caller: Caller,
    owner: PrincipalId,
}

impl Fixture {
    async fn start() -> Self {
        Self::start_with(Ok).await
    }

    async fn start_with(
        configure: impl FnOnce(Coordinator) -> Result<Coordinator, renoa_control::ControlError>,
    ) -> Self {
        let files = tempfile::tempdir().expect("directory");
        let coordinator = configure(
            Coordinator::open_with_passkeys(
                files.path().join("control.sqlite"),
                "localhost",
                ORIGIN,
            )
            .expect("coordinator"),
        )
        .expect("configured coordinator");
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("listener");
        let address = listener.local_addr().expect("address");
        let stop = CancellationToken::new();
        let task = tokio::spawn(coordinator.clone().serve(listener, stop.clone()));
        let owner = PrincipalId::from_uuid(Uuid::new_v4());
        let client = Client::new();
        let http = format!("http://{address}");
        let cookie = pair(&client, files.path(), &http, owner).await;
        Self {
            files,
            coordinator,
            ws: format!("ws://{address}/connect"),
            stop,
            task,
            caller: Caller {
                client,
                http,
                cookie: Some(cookie),
            },
            owner,
        }
    }

    async fn stop(self) {
        self.stop.cancel();
        self.task
            .await
            .expect("coordinator task")
            .expect("coordinator stops");
    }

    /// A signed-in browser of a principal that owns no node.
    async fn stranger(&self) -> Caller {
        Caller {
            cookie: Some(
                pair(
                    &self.caller.client,
                    self.files.path(),
                    &self.caller.http,
                    PrincipalId::from_uuid(Uuid::new_v4()),
                )
                .await,
            ),
            ..self.caller.clone()
        }
    }

    async fn enroll_node(&self) -> DeviceCredentials {
        self.enroll_node_of(self.owner).await
    }

    async fn enroll_node_of(&self, owner: PrincipalId) -> DeviceCredentials {
        let token = self
            .coordinator
            .create_node_enrollment(
                NodeId::new(),
                owner,
                SystemTime::now() + Duration::from_mins(1),
            )
            .await
            .expect("node enrollment");
        let (mut socket, _) = connect_async(&self.ws).await.expect("connect");
        send(
            &mut socket,
            &ClientMessage::Enroll {
                version: JSON_WS_VERSION,
                token,
            },
        )
        .await;
        let ServerMessage::Enrolled { credentials, .. } = receive(&mut socket).await else {
            panic!("the coordinator enrolls the node");
        };
        credentials
    }

    async fn connect(&self, credentials: &DeviceCredentials) -> Socket {
        let (mut socket, _) = connect_async(&self.ws).await.expect("connect");
        send(
            &mut socket,
            &ClientMessage::Authenticate {
                version: JSON_WS_VERSION,
                credentials: credentials.clone(),
            },
        )
        .await;
        assert_eq!(
            receive(&mut socket).await,
            ServerMessage::Authenticated {
                version: JSON_WS_VERSION
            }
        );
        socket
    }

    /// A connected node of the owner that has not said it serves Host control.
    async fn quiet_node(&self) -> Socket {
        let credentials = self.enroll_node().await;
        self.connect(&credentials).await
    }

    async fn serving_node(&self) -> Socket {
        let mut node = self.quiet_node().await;
        advertise(&mut node).await;
        node
    }
}

async fn pair(client: &Client, directory: &Path, http: &str, principal: PrincipalId) -> String {
    let token = BrowserSessions::open(directory.join("control.sqlite"))
        .expect("session issuer")
        .create_pairing(principal, SystemTime::now() + Duration::from_mins(30))
        .await
        .expect("pairing code");
    let response = client
        .post(format!("{http}/v1/identity/pair"))
        .header("origin", ORIGIN)
        .json(&json!({"pairingToken": token, "browserNonce": "78".repeat(32)}))
        .send()
        .await
        .expect("pair")
        .error_for_status()
        .expect("paired");
    response.headers()["set-cookie"]
        .to_str()
        .expect("cookie")
        .split(';')
        .next()
        .expect("cookie pair")
        .to_owned()
}

fn complete(call_id: Uuid, status: u16, body: &str) -> ClientMessage {
    ClientMessage::CompleteHostCall {
        call_id,
        status,
        body: body.to_owned(),
    }
}

async fn next_call(node: &mut Socket) -> (Uuid, PrincipalId, HostMethod, String, Option<String>) {
    let ServerMessage::HostCall {
        call_id,
        principal_id,
        method,
        path,
        body,
    } = receive(node).await
    else {
        panic!("the node is sent a Host call");
    };
    (call_id, principal_id, method, path, body)
}

fn spawn_answer(request: RequestBuilder) -> JoinHandle<Answer> {
    tokio::spawn(answer_of(request))
}

#[tokio::test]
async fn the_owners_call_reaches_the_node_that_serves_the_host_and_its_answer_returns() {
    let f = Fixture::start().await;
    let mut node = f.serving_node().await;
    let read = spawn_answer(f.caller.get("/v1/host"));

    let (call_id, principal, method, path, body) = next_call(&mut node).await;
    assert_eq!(
        (principal, method, path.as_str(), body),
        (f.owner, HostMethod::Get, "/v1/host", None)
    );
    send(&mut node, &complete(call_id, 200, r#"{"host_id":"h"}"#)).await;
    let answer = read.await.expect("read task");
    assert_eq!(answer.status, StatusCode::OK);
    assert_eq!(answer.json(), json!({"host_id": "h"}));
    assert_eq!(answer.headers["content-type"], "application/json");
    assert_eq!(answer.headers["cache-control"], "no-store");
    assert_eq!(answer.headers["x-content-type-options"], "nosniff");
    assert!(
        answer.headers["set-cookie"]
            .to_str()
            .expect("cookie")
            .starts_with("__Host-renoa_session="),
        "the browser's session is renewed with every answer"
    );

    // A write carries its body unchanged, and the node's status is the answer's.
    let body = json!({"operation_id": Uuid::nil(), "name": "Scout"});
    let write = spawn_answer(f.caller.write(Method::POST, "/v1/host/agents", &body));
    let (call_id, _, method, path, received) = next_call(&mut node).await;
    assert_eq!(
        (method, path.as_str()),
        (HostMethod::Post, "/v1/host/agents")
    );
    assert_eq!(
        serde_json::from_str::<Value>(&received.expect("a body")).expect("JSON body"),
        body
    );
    send(
        &mut node,
        &complete(
            call_id,
            409,
            r#"{"code":"operation_conflict","message":"m"}"#,
        ),
    )
    .await;
    let answer = write.await.expect("write task");
    assert_eq!(answer.status, StatusCode::CONFLICT);
    assert_eq!(answer.code(), "operation_conflict");

    let put = spawn_answer(f.caller.write(
        Method::PUT,
        "/v1/host/profile",
        &json!({"content": "x"}),
    ));
    let (call_id, _, method, path, _) = next_call(&mut node).await;
    assert_eq!(
        (method, path.as_str()),
        (HostMethod::Put, "/v1/host/profile")
    );
    send(&mut node, &complete(call_id, 200, "{}")).await;
    assert_eq!(put.await.expect("put task").status, StatusCode::OK);
    f.stop().await;
}

#[tokio::test]
async fn nothing_reaches_the_node_without_the_origin_the_session_and_the_owners_node() {
    let f = Fixture::start().await;
    let mut node = f.serving_node().await;
    let path = "/v1/host/automations/x/enabled";
    let body = json!({"enabled": false});
    let anonymous = f.caller.anonymous();

    let unsigned = answer_of(anonymous.get("/v1/host")).await;
    assert_eq!(unsigned.status, StatusCode::UNAUTHORIZED);
    assert_eq!(unsigned.code(), "sign_in_required");
    let spoofed = answer_of(
        anonymous
            .get("/v1/host")
            .header("x-renoa-principal", f.owner.to_string()),
    )
    .await;
    assert_eq!(spoofed.status, StatusCode::UNAUTHORIZED);
    assert_eq!(
        answer_of(anonymous.write(Method::POST, path, &body))
            .await
            .status,
        StatusCode::UNAUTHORIZED
    );

    for origin in [
        None,
        Some("null"),
        Some("https://evil.example"),
        Some("http://localhost.evil.example"),
        Some("http://localhost:1234"),
    ] {
        for caller in [&f.caller, &anonymous] {
            let mut request = caller
                .with_cookie(caller.client.post(format!("{}{path}", caller.http)))
                .json(&body);
            if let Some(origin) = origin {
                request = request.header("origin", origin);
            }
            let refused = answer_of(request).await;
            assert_eq!(refused.status, StatusCode::FORBIDDEN, "{origin:?}");
            assert_eq!(refused.code(), "wrong_origin");
        }
    }
    let doubled = f
        .caller
        .write(Method::POST, path, &body)
        .header("origin", ORIGIN);
    assert_eq!(answer_of(doubled).await.status, StatusCode::FORBIDDEN);

    let stranger = f.stranger().await;
    let foreign = answer_of(stranger.get("/v1/host")).await;
    assert_eq!(foreign.status, StatusCode::FORBIDDEN);
    assert_eq!(foreign.code(), "wrong_owner");

    for method in [Method::DELETE, Method::PATCH] {
        let refused = answer_of(f.caller.write(method, "/v1/host", &body)).await;
        assert_eq!(refused.status, StatusCode::METHOD_NOT_ALLOWED);
    }
    let text = f
        .caller
        .client
        .post(format!("{}/v1/host/agents", f.caller.http))
        .header("cookie", f.caller.cookie.as_deref().expect("cookie"))
        .header("origin", ORIGIN)
        .header("content-type", "text/plain")
        .body("not json");
    assert_eq!(
        answer_of(text).await.status,
        StatusCode::UNSUPPORTED_MEDIA_TYPE
    );
    // Over the 1 MiB read, and under it with no room left for the frame that
    // would carry it to the node.
    for length in [1024 * 1024, 1024 * 1024 - 16] {
        let huge = f.caller.write(
            Method::POST,
            "/v1/host/agents",
            &json!({"x": "a".repeat(length)}),
        );
        assert_eq!(answer_of(huge).await.status, StatusCode::PAYLOAD_TOO_LARGE);
    }

    BrowserSessions::open(f.files.path().join("control.sqlite"))
        .expect("session admin")
        .revoke(f.owner)
        .await
        .expect("revoke");
    assert_eq!(
        answer_of(f.caller.get("/v1/host")).await.status,
        StatusCode::UNAUTHORIZED
    );
    assert!(hears_nothing(&mut node).await, "the node was never called");
    f.stop().await;
}

#[tokio::test]
async fn a_host_that_is_offline_or_not_serving_is_unavailable_until_a_node_serves_it() {
    let f = Fixture::start().await;
    let credentials = f.enroll_node().await;
    let offline = answer_of(f.caller.get("/v1/host")).await;
    assert_eq!(offline.status, StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(offline.code(), "host_offline");
    assert_plain(&offline);

    // A connected node that never said it serves Host control is not called.
    let mut node = f.connect(&credentials).await;
    let silent = answer_of(f.caller.get("/v1/host")).await;
    assert_eq!(silent.code(), "host_offline");
    assert!(hears_nothing(&mut node).await);

    advertise(&mut node).await;
    let read = spawn_answer(f.caller.get("/v1/host"));
    let (call_id, ..) = next_call(&mut node).await;
    send(&mut node, &complete(call_id, 200, "{}")).await;
    assert_eq!(read.await.expect("read task").status, StatusCode::OK);

    // A new connection starts without the advertisement.
    let mut node = f.connect(&credentials).await;
    let reconnected = answer_of(f.caller.get("/v1/host")).await;
    assert_eq!(reconnected.code(), "host_offline");
    assert!(hears_nothing(&mut node).await);
    f.stop().await;
}

#[tokio::test]
async fn two_serving_nodes_of_one_owner_are_ambiguous_until_one_leaves() {
    let f = Fixture::start().await;
    let mut first = f.serving_node().await;
    let second = f.serving_node().await;
    let ambiguous = answer_of(f.caller.get("/v1/host")).await;
    assert_eq!(ambiguous.status, StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(ambiguous.code(), "host_ambiguous");
    assert_plain(&ambiguous);
    assert!(hears_nothing(&mut first).await);

    // The coordinator notices the departure asynchronously.
    drop(second);
    let caller = f.caller.clone();
    let read = tokio::spawn(async move {
        for _ in 0..500 {
            let answer = answer_of(caller.get("/v1/host")).await;
            if answer.status != StatusCode::CONFLICT {
                return answer;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        panic!("the second node never left");
    });
    let (call_id, ..) = next_call(&mut first).await;
    send(&mut first, &complete(call_id, 200, "{}")).await;
    assert_eq!(read.await.expect("read task").status, StatusCode::OK);
    f.stop().await;
}

#[tokio::test]
async fn a_node_that_disconnects_mid_call_ends_the_call() {
    let f = Fixture::start().await;
    let mut node = f.serving_node().await;
    let write = spawn_answer(f.caller.write(Method::POST, "/v1/host/agents", &json!({})));
    let _ = next_call(&mut node).await;
    node.close(None).await.expect("close the node");
    drop(node);

    let ended = write.await.expect("write task");
    assert_eq!(ended.status, StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(ended.code(), "host_disconnected");
    assert_plain(&ended);
    assert!(
        ended.json()["message"]
            .as_str()
            .expect("message")
            .contains("retry the same request"),
        "a change says how to recover its outcome"
    );
    f.stop().await;
}

#[tokio::test]
async fn a_connection_a_newer_one_replaced_still_ends_the_calls_sent_on_it() {
    let f = Fixture::start().await;
    let credentials = f.enroll_node().await;
    let mut old = f.connect(&credentials).await;
    advertise(&mut old).await;
    let write = spawn_answer(f.caller.write(Method::POST, "/v1/host/agents", &json!({})));
    let _ = next_call(&mut old).await;

    // The node returns on a new connection while the old one lingers. The old
    // connection's answers are dropped from then on, so its call ends when it
    // closes, not at the sixty-second deadline.
    let _current = f.connect(&credentials).await;
    old.close(None).await.expect("close the old connection");
    drop(old);

    let ended = timeout(Duration::from_secs(10), write)
        .await
        .expect("the call ends with its connection")
        .expect("write task");
    assert_eq!(ended.status, StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(ended.code(), "host_disconnected");
    f.stop().await;
}

#[tokio::test]
async fn a_login_storage_outage_is_unavailable_and_not_a_logout() {
    let f = Fixture::start().await;
    let mut node = f.serving_node().await;
    let storage =
        rusqlite::Connection::open(f.files.path().join("control.sqlite")).expect("fault injection");
    storage
        .execute("ALTER TABLE browser_sessions RENAME TO sessions_away", [])
        .expect("break login storage");

    let outage = answer_of(f.caller.get("/v1/host")).await;
    assert_eq!(outage.status, StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(outage.code(), "identity_unavailable");
    assert_plain(&outage);
    assert!(
        hears_nothing(&mut node).await,
        "no call is made on an unverified login"
    );

    // The browser keeps its cookie through the outage and is served again once
    // login storage returns.
    storage
        .execute("ALTER TABLE sessions_away RENAME TO browser_sessions", [])
        .expect("repair login storage");
    let read = spawn_answer(f.caller.get("/v1/host"));
    let (call_id, ..) = next_call(&mut node).await;
    send(&mut node, &complete(call_id, 200, "{}")).await;
    assert_eq!(read.await.expect("read task").status, StatusCode::OK);
    f.stop().await;
}

#[tokio::test]
async fn a_failed_lookup_of_the_owners_host_is_unavailable_in_plain_words() {
    let f = Fixture::start().await;
    let mut node = f.serving_node().await;
    let storage =
        rusqlite::Connection::open(f.files.path().join("control.sqlite")).expect("fault injection");
    storage
        .execute("ALTER TABLE node_owners RENAME TO owners_away", [])
        .expect("break ownership storage");

    let failed = answer_of(f.caller.get("/v1/host")).await;
    assert_eq!(failed.status, StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(failed.code(), "host_unavailable");
    assert_plain(&failed);
    assert!(hears_nothing(&mut node).await);
    f.stop().await;
}

#[tokio::test]
async fn only_the_nodes_own_first_answer_to_a_call_is_delivered() {
    let f = Fixture::start().await;
    let mut serving = f.serving_node().await;
    let mut bystander = f.quiet_node().await;
    let mut read = spawn_answer(f.caller.get("/v1/host"));
    let (call_id, ..) = next_call(&mut serving).await;

    // Another node of the owner cannot answer a call it was not sent.
    send(
        &mut bystander,
        &complete(call_id, 200, r#"{"forged":true}"#),
    )
    .await;
    assert!(
        timeout(QUIET, &mut read).await.is_err(),
        "the call stays pending"
    );
    assert!(
        hears_nothing(&mut bystander).await,
        "a foreign answer is dropped silently"
    );

    send(&mut serving, &complete(call_id, 200, r#"{"real":true}"#)).await;
    let answer = read.await.expect("read task");
    assert_eq!(answer.json(), json!({"real": true}));

    // A repeat, or an answer to a call nobody waits for, is dropped and the
    // link stays usable.
    send(&mut serving, &complete(call_id, 200, r#"{"again":true}"#)).await;
    send(&mut serving, &complete(Uuid::new_v4(), 200, "{}")).await;
    let next = spawn_answer(f.caller.get("/v1/host"));
    let (second, ..) = next_call(&mut serving).await;
    assert_ne!(second, call_id);
    send(&mut serving, &complete(second, 200, r#"{"second":true}"#)).await;
    assert_eq!(
        next.await.expect("next task").json(),
        json!({"second": true})
    );
    f.stop().await;
}

#[tokio::test]
async fn an_answer_the_relay_cannot_forward_is_a_bad_gateway() {
    let f = Fixture::start().await;
    let mut node = f.serving_node().await;
    for status in [99_u16, 199, 302, 600, 1000] {
        let call = spawn_answer(f.caller.get("/v1/host"));
        let (call_id, ..) = next_call(&mut node).await;
        send(&mut node, &complete(call_id, status, "{}")).await;
        let bad = call.await.expect("call task");
        assert_eq!(bad.status, StatusCode::BAD_GATEWAY, "{status}");
        assert_eq!(bad.code(), "host_invalid_reply");
        assert_plain(&bad);
    }
    f.stop().await;
}

#[tokio::test]
async fn only_a_node_connection_can_advertise_or_answer_for_a_host() {
    let f = Fixture::start().await;

    // A surface of the owner cannot.
    let token = f
        .coordinator
        .create_enrollment(
            PeerIdentity::Surface {
                principal_id: f.owner,
                surface: SurfaceRef::new("phone"),
            },
            SystemTime::now() + Duration::from_mins(1),
        )
        .await
        .expect("surface enrollment");
    let (mut socket, _) = connect_async(&f.ws).await.expect("connect");
    send(
        &mut socket,
        &ClientMessage::Enroll {
            version: JSON_WS_VERSION,
            token,
        },
    )
    .await;
    let ServerMessage::Enrolled { credentials, .. } = receive(&mut socket).await else {
        panic!("the coordinator enrolls the surface");
    };
    let mut surface = f.connect(&credentials).await;
    send(&mut surface, &ClientMessage::ServeHostControl).await;
    assert!(matches!(
        receive(&mut surface).await,
        ServerMessage::Error {
            code: ErrorCode::InvalidRole,
            ..
        }
    ));
    send(&mut surface, &complete(Uuid::new_v4(), 200, "{}")).await;
    assert!(matches!(
        receive(&mut surface).await,
        ServerMessage::Error {
            code: ErrorCode::InvalidRole,
            ..
        }
    ));

    // Neither can a connection that has not yet said who it is.
    let (mut anonymous, _) = connect_async(&f.ws).await.expect("connect");
    send(&mut anonymous, &ClientMessage::ServeHostControl).await;
    assert!(matches!(
        receive(&mut anonymous).await,
        ServerMessage::Error {
            code: ErrorCode::InvalidMessage,
            ..
        }
    ));

    // A node connection that a newer one replaced cannot.
    let credentials = f.enroll_node().await;
    let mut replaced = f.connect(&credentials).await;
    let mut current = f.connect(&credentials).await;
    send(&mut replaced, &ClientMessage::ServeHostControl).await;
    assert!(matches!(
        receive(&mut replaced).await,
        ServerMessage::Error {
            code: ErrorCode::InvalidRole,
            ..
        }
    ));
    let silent = answer_of(f.caller.get("/v1/host")).await;
    assert_eq!(
        silent.code(),
        "host_offline",
        "the replaced node advertised nothing"
    );
    advertise(&mut current).await;
    let read = spawn_answer(f.caller.get("/v1/host"));
    let (call_id, ..) = next_call(&mut current).await;
    send(&mut current, &complete(call_id, 200, "{}")).await;
    assert_eq!(read.await.expect("read task").status, StatusCode::OK);
    f.stop().await;
}

fn files_containing(directory: &Path, marker: &str) -> Vec<String> {
    std::fs::read_dir(directory)
        .expect("directory")
        .filter_map(Result::ok)
        .filter(|entry| entry.path().is_file())
        .filter(|entry| {
            std::fs::read(entry.path())
                .is_ok_and(|bytes| bytes.windows(marker.len()).any(|w| w == marker.as_bytes()))
        })
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
        .collect()
}

#[tokio::test]
async fn a_calls_request_and_answer_are_not_stored_by_the_coordinator() {
    let f = Fixture::start().await;
    let mut node = f.serving_node().await;
    let secret = "bot-token-0123456789-marker";
    let reply = "reply-body-9876543210-marker";

    // The scan finds a marker where one is written, so a clean result means something.
    std::fs::write(f.files.path().join("control"), secret).expect("control file");
    assert_eq!(files_containing(f.files.path(), secret), ["control"]);
    std::fs::remove_file(f.files.path().join("control")).expect("remove control file");

    let sent = json!({"bot_token": secret});
    let call = spawn_answer(
        f.caller
            .write(Method::POST, "/v1/host/discord/inspection", &sent),
    );
    let (call_id, _, _, _, body) = next_call(&mut node).await;
    assert!(body.expect("a body").contains(secret));
    send(
        &mut node,
        &complete(call_id, 200, &format!(r#"{{"reply":"{reply}"}}"#)),
    )
    .await;
    assert_eq!(call.await.expect("call task").status, StatusCode::OK);

    assert!(files_containing(f.files.path(), secret).is_empty());
    assert!(files_containing(f.files.path(), reply).is_empty());
    f.stop().await;
}

#[tokio::test]
async fn the_login_page_learns_the_one_owner_of_the_enrolled_nodes() {
    let f = Fixture::start().await;
    let anonymous = f.caller.anonymous();
    let none = answer_of(anonymous.get("/v1/host/access")).await;
    assert_eq!(none.status, StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(none.code(), "owner_unknown");
    assert_plain(&none);

    // An enrollment nobody claimed is not an enrolled node, even the owner's.
    f.coordinator
        .create_node_enrollment(
            NodeId::new(),
            f.owner,
            SystemTime::now() + Duration::from_mins(1),
        )
        .await
        .expect("an enrollment nobody claims");
    assert_eq!(
        answer_of(anonymous.get("/v1/host/access")).await.code(),
        "owner_unknown"
    );

    let first = f.enroll_node().await;
    let second = f.enroll_node().await;
    let one = answer_of(anonymous.get("/v1/host/access")).await;
    assert_eq!(one.status, StatusCode::OK);
    assert_eq!(one.json(), json!({"owner_principal_id": f.owner}));
    assert_eq!(one.headers["cache-control"], "no-store");

    // A stray enrollment for another principal must not lock the owner out of
    // signing in: only a claimed node counts.
    let stranger = PrincipalId::from_uuid(Uuid::new_v4());
    f.coordinator
        .create_node_enrollment(
            NodeId::new(),
            stranger,
            SystemTime::now() + Duration::from_mins(1),
        )
        .await
        .expect("an enrollment nobody claims");
    let unclaimed = answer_of(anonymous.get("/v1/host/access")).await;
    assert_eq!(unclaimed.json(), json!({"owner_principal_id": f.owner}));

    // Once another principal's node is enrolled the owner is no longer unique;
    // when that node is revoked it is again.
    let strangers_node = f.enroll_node_of(stranger).await;
    let several = answer_of(anonymous.get("/v1/host/access")).await;
    assert_eq!(several.status, StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(several.code(), "owner_unknown");
    f.coordinator
        .revoke_device(strangers_node.device_id)
        .await
        .expect("revoke the stranger's node");
    let revoked = answer_of(anonymous.get("/v1/host/access")).await;
    assert_eq!(revoked.json(), json!({"owner_principal_id": f.owner}));

    // A revoked node names no one: with every node revoked there is no owner.
    for credentials in [first, second] {
        f.coordinator
            .revoke_device(credentials.device_id)
            .await
            .expect("revoke the owner's node");
    }
    assert_eq!(
        answer_of(anonymous.get("/v1/host/access")).await.code(),
        "owner_unknown"
    );
    f.stop().await;
}

#[tokio::test]
async fn the_control_room_is_served_beside_the_api_with_no_stale_cache() {
    let assets = tempfile::tempdir().expect("assets");
    std::fs::create_dir(assets.path().join("assets")).expect("assets directory");
    std::fs::write(assets.path().join("index.html"), "control panel").expect("shell");
    std::fs::write(assets.path().join("assets/app-1.js"), "app").expect("script");
    let directory = assets.path().to_owned();
    let f = Fixture::start_with(move |coordinator| coordinator.with_control_room(&directory)).await;
    let anonymous = f.caller.anonymous();

    let shell = answer_of(anonymous.get("/")).await;
    assert_eq!(
        (shell.status, shell.text.as_str()),
        (StatusCode::OK, "control panel")
    );
    assert_eq!(shell.headers["cache-control"], "no-store");
    assert_eq!(shell.headers["x-content-type-options"], "nosniff");
    assert_eq!(shell.headers["referrer-policy"], "no-referrer");
    assert!(
        !shell.headers.contains_key("content-security-policy"),
        "the app shell must not inherit the API's script-less policy"
    );
    assert_eq!(
        answer_of(anonymous.get("/assets/app-1.js")).await.text,
        "app"
    );
    for path in ["/v1/unknown", "/assets/unknown.js"] {
        let missing = answer_of(anonymous.get(path).header("accept", "text/html")).await;
        assert_eq!(missing.status, StatusCode::NOT_FOUND, "{path}");
    }
    let write = anonymous
        .client
        .post(format!("{}/", anonymous.http))
        .send()
        .await
        .expect("write to the shell");
    assert_eq!(write.status(), StatusCode::METHOD_NOT_ALLOWED);
    // The API still answers on the same origin.
    assert_eq!(
        answer_of(anonymous.get("/v1/host")).await.status,
        StatusCode::UNAUTHORIZED
    );
    f.stop().await;
}

#[cfg(unix)]
#[tokio::test]
async fn a_release_switch_is_served_without_restarting_the_coordinator() {
    let releases = tempfile::tempdir().expect("releases");
    for release in ["v1", "v2"] {
        std::fs::create_dir(releases.path().join(release)).expect("release");
        std::fs::write(releases.path().join(release).join("index.html"), release).expect("shell");
    }
    let current = releases.path().join("control-room");
    std::os::unix::fs::symlink(releases.path().join("v1"), &current).expect("link v1");
    let served = current.clone();
    let f = Fixture::start_with(move |coordinator| coordinator.with_control_room(&served)).await;
    let anonymous = f.caller.anonymous();
    assert_eq!(answer_of(anonymous.get("/")).await.text, "v1");

    let next = releases.path().join("control-room.next");
    std::os::unix::fs::symlink(releases.path().join("v2"), &next).expect("link v2");
    std::fs::rename(&next, &current).expect("switch release");
    assert_eq!(answer_of(anonymous.get("/")).await.text, "v2");
    f.stop().await;
}

#[tokio::test]
async fn a_control_room_directory_without_a_shell_is_refused() {
    let empty = tempfile::tempdir().expect("directory");
    let files = tempfile::tempdir().expect("files");
    let coordinator =
        Coordinator::open_with_passkeys(files.path().join("control.sqlite"), "localhost", ORIGIN)
            .expect("coordinator");
    assert!(coordinator.with_control_room(empty.path()).is_err());
}

// --- `call_host`: the same relay over an RCP connection ---

impl Fixture {
    /// A browser's RCP connection, opened with a ticket from its remembered session.
    async fn browser_connection(&self, caller: &Caller) -> Socket {
        let ticket: Value = caller
            .write(
                Method::POST,
                "/v1/identity/connection-ticket",
                &json!({"surface": "control_room"}),
            )
            .send()
            .await
            .expect("ticket request")
            .error_for_status()
            .expect("a ticket")
            .json()
            .await
            .expect("ticket JSON");
        let (mut socket, _) = connect_async(&self.ws).await.expect("connect");
        send(
            &mut socket,
            &ClientMessage::AuthenticateTicket {
                version: JSON_WS_VERSION,
                ticket: serde_json::from_value(ticket["connectionTicket"].clone()).expect("ticket"),
            },
        )
        .await;
        assert!(matches!(
            receive(&mut socket).await,
            ServerMessage::Authenticated { .. }
        ));
        socket
    }

    /// A chat surface of the owner, linked with a bearer credential as Discord is.
    async fn chat_link(&self) -> Socket {
        let token = self
            .coordinator
            .create_enrollment(
                PeerIdentity::Surface {
                    principal_id: self.owner,
                    surface: SurfaceRef::new("discord"),
                },
                SystemTime::now() + Duration::from_mins(1),
            )
            .await
            .expect("surface enrollment");
        let (mut socket, _) = connect_async(&self.ws).await.expect("connect");
        send(
            &mut socket,
            &ClientMessage::Enroll {
                version: JSON_WS_VERSION,
                token,
            },
        )
        .await;
        let ServerMessage::Enrolled { credentials, .. } = receive(&mut socket).await else {
            panic!("the coordinator enrolls the surface");
        };
        self.connect(&credentials).await
    }
}

fn call_host(request_id: u64, method: HostMethod, path: &str, body: Option<&str>) -> ClientMessage {
    ClientMessage::CallHost {
        request_id,
        method,
        path: path.to_owned(),
        body: body.map(str::to_owned),
    }
}

/// The error a request was answered with, and its message.
async fn refusal(socket: &mut Socket, request_id: u64) -> (ErrorCode, String) {
    match receive(socket).await {
        ServerMessage::Error {
            request_id: Some(answered),
            code,
            message,
        } if answered == request_id => (code, message),
        other => panic!("expected request {request_id} to be refused, received {other:?}"),
    }
}

#[tokio::test]
async fn only_a_signed_in_app_calls_the_host_over_rcp_never_a_browser_ticket_or_a_chat_link() {
    let f = Fixture::start().await;
    let mut node = f.serving_node().await;
    // A ticket connection outlives the browser session that issued it, so it
    // would keep Host control after a logout; the browser uses /v1/host.
    let mut browser = f.browser_connection(&f.caller).await;
    let mut link = f.chat_link().await;
    for socket in [&mut browser, &mut link] {
        send(socket, &call_host(1, HostMethod::Get, "/v1/host", None)).await;
        let (code, message) = refusal(socket, 1).await;
        assert_eq!(code, ErrorCode::InvalidRole);
        assert!(message.contains("signed-in app"), "{message}");
    }
    assert!(hears_nothing(&mut node).await);
    // The same browser still reaches its Host through its session.
    let read = spawn_answer(f.caller.get("/v1/host"));
    let (call_id, ..) = next_call(&mut node).await;
    send(&mut node, &complete(call_id, 200, "{}")).await;
    assert_eq!(read.await.expect("read task").status, StatusCode::OK);
    f.stop().await;
}

impl Fixture {
    /// The owner's phone, signed in with a device key as the app is.
    async fn phone(&self) -> Socket {
        use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
        use ring::{
            rand::SystemRandom,
            signature::{ECDSA_P256_SHA256_ASN1_SIGNING, EcdsaKeyPair, KeyPair as _},
        };

        let token = self
            .coordinator
            .create_enrollment(
                PeerIdentity::Surface {
                    principal_id: self.owner,
                    surface: SurfaceRef::new("android"),
                },
                SystemTime::now() + Duration::from_mins(1),
            )
            .await
            .expect("app enrollment");
        let (mut socket, _) = connect_async(&self.ws).await.expect("connect");
        send(
            &mut socket,
            &ClientMessage::Enroll {
                version: JSON_WS_VERSION,
                token,
            },
        )
        .await;
        let ServerMessage::Enrolled { credentials, .. } = receive(&mut socket).await else {
            panic!("the coordinator enrolls the app");
        };
        let rng = SystemRandom::new();
        let pkcs8 =
            EcdsaKeyPair::generate_pkcs8(&ECDSA_P256_SHA256_ASN1_SIGNING, &rng).expect("key");
        let key = EcdsaKeyPair::from_pkcs8(&ECDSA_P256_SHA256_ASN1_SIGNING, pkcs8.as_ref(), &rng)
            .expect("load key");
        rusqlite::Connection::open(self.files.path().join("control.sqlite"))
            .expect("control database")
            .execute(
                "INSERT INTO device_keys (device_id, principal_id, public_key, label, created_at_ms)
                 VALUES (?1, ?2, ?3, 'phone', 0)",
                rusqlite::params![
                    credentials.device_id.to_string(),
                    self.owner.to_string(),
                    key.public_key().as_ref(),
                ],
            )
            .expect("bind the device key");
        let (mut socket, _) = connect_async(&self.ws).await.expect("connect");
        send(
            &mut socket,
            &ClientMessage::Authenticate {
                version: JSON_WS_VERSION,
                credentials: credentials.clone(),
            },
        )
        .await;
        let ServerMessage::DeviceChallenge { nonce } = receive(&mut socket).await else {
            panic!("a key-bound device is challenged");
        };
        let proof = format!("renoa device proof v1\n{}\n{nonce}", credentials.device_id);
        let signature = key.sign(&rng, proof.as_bytes()).expect("sign");
        send(
            &mut socket,
            &ClientMessage::DeviceProof {
                signature: URL_SAFE_NO_PAD.encode(signature.as_ref()),
            },
        )
        .await;
        assert!(matches!(
            receive(&mut socket).await,
            ServerMessage::Authenticated { .. }
        ));
        socket
    }
}

const PAGE: &[u8] = b"<p>hi</p>";

/// The Host's description of `PAGE` shared as `page.html`, in parts of 4 bytes.
fn page_description() -> String {
    let digest = ring::digest::digest(&ring::digest::SHA256, PAGE);
    let sha256 = digest.as_ref().iter().fold(String::new(), |mut hex, byte| {
        use std::fmt::Write as _;
        let _ = write!(hex, "{byte:02x}");
        hex
    });
    json!({
        "name": "my page.html", "media_type": "text/html", "size": PAGE.len(),
        "sha256": sha256, "part_bytes": 4, "parts": 3
    })
    .to_string()
}

/// Answers the node's calls for share `share` as the Host does, with `parts`
/// as the bytes of each part, and checks each call is the owner's.
async fn serve_share(node: &mut Socket, owner: PrincipalId, share: Uuid, parts: &[&[u8]]) {
    use base64::{Engine as _, engine::general_purpose::STANDARD};

    let (call_id, principal, method, path, _) = next_call(node).await;
    assert_eq!(
        (principal, method, path),
        (owner, HostMethod::Get, format!("/v1/host/shares/{share}"))
    );
    send(node, &complete(call_id, 200, &page_description())).await;
    for (index, part) in parts.iter().enumerate() {
        let (call_id, _, _, path, _) = next_call(node).await;
        assert_eq!(path, format!("/v1/host/shares/{share}/parts/{index}"));
        send(
            node,
            &complete(
                call_id,
                200,
                &json!({"data": STANDARD.encode(part)}).to_string(),
            ),
        )
        .await;
    }
}

async fn share_link(phone: &mut Socket, share: Uuid) -> String {
    send(
        phone,
        &ClientMessage::ShareLink {
            request_id: 7,
            share_id: share,
        },
    )
    .await;
    match receive(phone).await {
        ServerMessage::ShareLinkIssued {
            request_id: 7,
            path,
        } => path.expose().to_owned(),
        other => panic!("expected a share link, received {other:?}"),
    }
}

#[tokio::test]
async fn a_phones_share_link_opens_the_file_once_sandboxed() {
    let f = Fixture::start().await;
    let mut node = f.serving_node().await;
    let mut phone = f.phone().await;
    let share = Uuid::new_v4();
    let path = share_link(&mut phone, share).await;
    assert!(path.starts_with(&format!("/s/{share}?ticket=")), "{path}");

    let anonymous = f.caller.anonymous();
    let opened = spawn_answer(anonymous.get(&path));
    serve_share(&mut node, f.owner, share, &[b"<p>h", b"i</p", b">"]).await;
    let opened = opened.await.expect("open task");
    assert_eq!(opened.status, StatusCode::OK);
    assert_eq!(opened.text.as_bytes(), PAGE);
    let header = |name: &str| opened.headers[name].to_str().expect("header").to_owned();
    assert_eq!(header("content-type"), "text/html; charset=utf-8");
    assert_eq!(
        header("content-disposition"),
        "inline; filename*=UTF-8''my%20page.html"
    );
    // Scripts only, in an opaque origin: no same-origin, top navigation,
    // escaping popups, forms or dialogs.
    assert_eq!(
        header("content-security-policy"),
        "sandbox allow-scripts; frame-ancestors 'none'"
    );
    assert_eq!(header("cache-control"), "private, no-store, no-transform");
    assert_eq!(header("x-content-type-options"), "nosniff");
    assert_eq!(header("referrer-policy"), "no-referrer");
    assert_eq!(header("cross-origin-resource-policy"), "same-origin");
    assert!(!opened.headers.contains_key("set-cookie"));

    let again = answer_of(anonymous.get(&path)).await;
    assert_eq!(again.status, StatusCode::UNAUTHORIZED, "a link opens once");
    assert!(again.text.contains("already opened"), "{}", again.text);
    let other = share_link(&mut phone, share)
        .await
        .replace(&share.to_string(), &Uuid::new_v4().to_string());
    assert_eq!(
        answer_of(anonymous.get(&other)).await.status,
        StatusCode::UNAUTHORIZED,
        "a link is for its own share"
    );
    assert!(hears_nothing(&mut node).await);
    f.stop().await;
}

#[tokio::test]
async fn a_signed_in_browser_opens_a_share_and_no_one_else_does() {
    let f = Fixture::start().await;
    let mut node = f.serving_node().await;
    let share = Uuid::new_v4();
    let opened = spawn_answer(f.caller.get(&format!("/s/{share}")));
    serve_share(&mut node, f.owner, share, &[b"<p>h", b"i</p", b">"]).await;
    let opened = opened.await.expect("open task");
    assert_eq!(
        (opened.status, opened.text.as_bytes()),
        (StatusCode::OK, PAGE)
    );
    assert!(
        opened.headers.contains_key("set-cookie"),
        "the session is refreshed"
    );

    let anonymous = f.caller.anonymous();
    let signed_out = answer_of(anonymous.get(&format!("/s/{share}"))).await;
    assert_eq!(signed_out.status, StatusCode::UNAUTHORIZED);
    assert!(
        signed_out.text.starts_with("Sign in"),
        "{}",
        signed_out.text
    );
    for path in [
        format!("/s/{share}?ticket=nothex"),
        format!("/s/{share}?ticket={}", "a".repeat(64)),
    ] {
        assert_eq!(
            answer_of(f.caller.get(&path)).await.status,
            StatusCode::UNAUTHORIZED,
            "{path}"
        );
    }
    let head = answer_of(f.caller.client.head(format!("{}/s/{share}", f.caller.http))).await;
    assert_eq!(head.status, StatusCode::METHOD_NOT_ALLOWED);
    assert_eq!(
        answer_of(f.caller.get(&format!("/s/{}", share.simple())))
            .await
            .status,
        StatusCode::NOT_FOUND
    );
    assert!(hears_nothing(&mut node).await);

    // The Host's refusal is the page, in its words and status.
    let gone = spawn_answer(f.caller.get(&format!("/s/{share}")));
    let (call_id, ..) = next_call(&mut node).await;
    send(
        &mut node,
        &complete(
            call_id,
            404,
            r#"{"code":"share_not_found","message":"Nothing is shared at this address any more."}"#,
        ),
    )
    .await;
    let gone = gone.await.expect("gone task");
    assert_eq!(
        (gone.status, gone.text.as_str()),
        (
            StatusCode::NOT_FOUND,
            "Nothing is shared at this address any more."
        )
    );
    f.stop().await;
}

#[tokio::test]
async fn a_file_that_changes_while_it_is_read_is_cut_short() {
    let f = Fixture::start().await;
    let mut node = f.serving_node().await;
    let share = Uuid::new_v4();
    for parts in [
        &[b"<p>h" as &[u8], b"i</p", b"!"][..],
        &[b"<p>h" as &[u8], b"i</p>x"][..],
    ] {
        let response = tokio::spawn(f.caller.get(&format!("/s/{share}")).send());
        serve_share(&mut node, f.owner, share, parts).await;
        let response = response.await.expect("task").expect("headers arrive");
        assert_eq!(response.status(), StatusCode::OK);
        assert!(
            response.bytes().await.is_err(),
            "a changed file never reads as whole"
        );
    }
    f.stop().await;
}

#[tokio::test]
async fn only_a_signed_in_app_gets_a_share_link() {
    let f = Fixture::start().await;
    let mut browser = f.browser_connection(&f.caller).await;
    let mut link = f.chat_link().await;
    let mut node = f.serving_node().await;
    send(
        &mut node,
        &ClientMessage::ShareLink {
            request_id: 3,
            share_id: Uuid::new_v4(),
        },
    )
    .await;
    assert_eq!(refusal(&mut node, 3).await.0, ErrorCode::InvalidRole);
    for socket in [&mut browser, &mut link] {
        send(
            socket,
            &ClientMessage::ShareLink {
                request_id: 3,
                share_id: Uuid::new_v4(),
            },
        )
        .await;
        let (code, message) = refusal(socket, 3).await;
        assert_eq!(code, ErrorCode::InvalidRole);
        assert!(message.contains("signed-in app"), "{message}");
    }
    f.stop().await;
}

/// A link is used up only once its file is known readable: a HEAD, a Host
/// that is away and a file that changed while it opened all leave it working.
#[tokio::test]
async fn a_share_link_survives_what_the_reader_can_retry() {
    let f = Fixture::start().await;
    let mut node = f.serving_node().await;
    let mut phone = f.phone().await;
    let share = Uuid::new_v4();
    let path = share_link(&mut phone, share).await;
    let anonymous = f.caller.anonymous();
    let head = answer_of(anonymous.client.head(format!("{}{path}", anonymous.http))).await;
    assert_eq!(head.status, StatusCode::METHOD_NOT_ALLOWED);

    let away = spawn_answer(anonymous.get(&path));
    let (call_id, ..) = next_call(&mut node).await;
    send(&mut node, &complete(call_id, 503, r#"{"code":"host_unavailable","message":"The shared file could not be read. Try again in a moment."}"#)).await;
    assert_eq!(
        away.await.expect("away task").status,
        StatusCode::SERVICE_UNAVAILABLE
    );

    let changed = spawn_answer(anonymous.get(&path));
    serve_share(&mut node, f.owner, share, &[b"<p>hi"]).await;
    let changed = changed.await.expect("changed task");
    assert_eq!(changed.status, StatusCode::CONFLICT, "{}", changed.text);

    let opened = spawn_answer(anonymous.get(&path));
    serve_share(&mut node, f.owner, share, &[b"<p>h", b"i</p", b">"]).await;
    assert_eq!(opened.await.expect("open task").text.as_bytes(), PAGE);
    f.stop().await;
}

/// Two opens of one link at once: both may read, but one alone is served.
#[tokio::test]
async fn a_share_link_opened_twice_at_once_serves_once() {
    use base64::{Engine as _, engine::general_purpose::STANDARD};
    let f = Fixture::start().await;
    let mut node = f.serving_node().await;
    let mut phone = f.phone().await;
    let share = Uuid::new_v4();
    let path = share_link(&mut phone, share).await;
    let anonymous = f.caller.anonymous();
    let first = spawn_answer(anonymous.get(&path));
    let second = spawn_answer(anonymous.get(&path));
    // Each open reads the description and its first part before using the link.
    for _ in 0..4 {
        let (call_id, _, _, path, _) = next_call(&mut node).await;
        let body = if path.ends_with("/parts/0") {
            json!({"data": "PHA+aA=="}).to_string()
        } else {
            page_description()
        };
        send(&mut node, &complete(call_id, 200, &body)).await;
    }
    // The one served goes on to its other parts.
    for part in [b"i</p" as &[u8], b">"] {
        let (call_id, ..) = next_call(&mut node).await;
        send(
            &mut node,
            &complete(
                call_id,
                200,
                &json!({"data": STANDARD.encode(part)}).to_string(),
            ),
        )
        .await;
    }
    let mut statuses = [
        first.await.expect("first").status,
        second.await.expect("second").status,
    ];
    statuses.sort();
    assert_eq!(statuses, [StatusCode::OK, StatusCode::UNAUTHORIZED]);
    f.stop().await;
}

#[tokio::test]
async fn a_browser_link_with_another_query_still_opens_with_its_sign_in() {
    let f = Fixture::start().await;
    let mut node = f.serving_node().await;
    let share = Uuid::new_v4();
    let opened = spawn_answer(f.caller.get(&format!("/s/{share}?from=chat")));
    serve_share(&mut node, f.owner, share, &[b"<p>h", b"i</p", b">"]).await;
    assert_eq!(opened.await.expect("open task").status, StatusCode::OK);
    f.stop().await;
}
