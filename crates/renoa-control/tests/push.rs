//! Content-free push wakes, through a real coordinator over WebSocket, its
//! SQLite file, and a local stand-in for Google's token endpoint and FCM.

use std::{
    collections::{HashMap, VecDeque},
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    time::{Duration, SystemTime},
};

use axum::{
    Router,
    body::Bytes,
    extract::State,
    http::{HeaderMap, Method, StatusCode, Uri},
    response::{IntoResponse, Response},
};
use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use futures_util::{SinkExt, StreamExt};
use renoa_control::{
    BrowserSessions, ClientMessage, Coordinator, DeviceCredentials, ErrorCode, JSON_WS_VERSION,
    NodeId, PeerIdentity, PushSettings, PushToken, ServerMessage, TaskId, TaskSpec, WakeUrgency,
};
use renoa_protocol::{
    Author, CommandId, CommandInput, ExecutionEvent, ExecutionEventId, ExecutionEventKind,
    ExecutionId, ExecutionTerminal, PrincipalId, SurfaceRef, TargetRef,
};
use rusqlite::Connection;
use serde_json::{Value, json};
use tempfile::TempDir;
use tokio::{
    net::TcpListener,
    sync::watch,
    task::JoinHandle,
    time::{Instant, timeout},
};
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream, connect_async, tungstenite::Message};
use tokio_util::sync::CancellationToken;
use url::Url;
use uuid::Uuid;

type Socket = WebSocketStream<MaybeTlsStream<tokio::net::TcpStream>>;

/// A key generated for these tests only; it signs nothing anywhere else.
const TEST_KEY: &str = include_str!("support/push-test-key.pem");
const PROJECT: &str = "renoa-test";
const ACCESS_TOKEN: &str = "ya29.stub-access-token-for-push-tests";
const ORIGIN: &str = "http://localhost";
/// How long a test waits to see that nothing more is sent.
const QUIET: Duration = Duration::from_millis(600);
const WAIT: Duration = Duration::from_secs(10);

// --- The stand-in for Google ---

#[derive(Debug, Clone)]
struct Recorded {
    method: String,
    path: String,
    headers: Vec<(String, String)>,
    body: String,
    at: Instant,
}

impl Recorded {
    fn is_fcm(&self) -> bool {
        self.path.ends_with("/messages:send")
    }

    /// Everything a request carries, for checks that something is absent.
    fn everything(&self) -> String {
        let headers: Vec<String> = self
            .headers
            .iter()
            .map(|(name, value)| format!("{name}: {value}"))
            .collect();
        format!(
            "{} {}\n{}\n{}",
            self.method,
            self.path,
            headers.join("\n"),
            self.body
        )
    }

    fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(header, _)| header == name)
            .map(|(_, value)| value.as_str())
    }

    /// The device token a wake was sent to.
    fn token(&self) -> Option<String> {
        let body: Value = serde_json::from_str(&self.body).ok()?;
        body["message"]["token"].as_str().map(ToOwned::to_owned)
    }
}

/// How many wakes `requests` sent to `token`.
fn wakes_to(requests: &[Recorded], token: &str) -> usize {
    requests
        .iter()
        .filter(|request| request.is_fcm() && request.token().as_deref() == Some(token))
        .count()
}

#[derive(Clone)]
struct Answer {
    status: u16,
    body: Value,
    retry_after: Option<u64>,
}

impl Answer {
    fn fcm_error(status: u16, code: &str, canonical: &str) -> Self {
        Self {
            status,
            body: json!({
                "error": {
                    "code": status,
                    "message": "stub error",
                    "status": canonical,
                    "details": [{
                        "@type": "type.googleapis.com/google.firebase.fcm.v1.FcmError",
                        "errorCode": code,
                    }],
                }
            }),
            retry_after: None,
        }
    }

    /// A 400 whose only detail is a `BadRequest` violation on the token.
    fn token_violation() -> Self {
        Self {
            status: 400,
            body: json!({
                "error": {
                    "code": 400,
                    "message": "stub error",
                    "status": "INVALID_ARGUMENT",
                    "details": [{
                        "@type": "type.googleapis.com/google.rpc.BadRequest",
                        "fieldViolations": [{
                            "field": "message.token",
                            "description": "Invalid registration token",
                        }],
                    }],
                }
            }),
            retry_after: None,
        }
    }

    /// A 400 whose `FcmError` is `INVALID_ARGUMENT` and whose `BadRequest`
    /// violation is on the token.
    fn invalid_argument_on_the_token() -> Self {
        let mut answer = Self::token_violation();
        answer.body["error"]["details"]
            .as_array_mut()
            .expect("details")
            .push(json!({
                "@type": "type.googleapis.com/google.firebase.fcm.v1.FcmError",
                "errorCode": "INVALID_ARGUMENT",
            }));
        answer
    }

    /// An answer with an empty JSON body.
    fn status(status: u16) -> Self {
        Self {
            status,
            body: json!({}),
            retry_after: None,
        }
    }
}

#[derive(Default)]
struct StubState {
    requests: Mutex<Vec<Recorded>>,
    token_answers: Mutex<VecDeque<Answer>>,
    fcm_answers: Mutex<VecDeque<Answer>>,
    /// Answers for wakes to one token, used before `fcm_answers`.
    fcm_answers_for: Mutex<HashMap<String, VecDeque<Answer>>>,
    count: Option<watch::Sender<usize>>,
    /// A request to a gated FCM token, or to a gated path such as `/token`,
    /// is recorded, then answered only once its gate is free, so a test can
    /// act while that request is in flight.
    gates: Mutex<HashMap<String, Arc<tokio::sync::Mutex<()>>>>,
}

struct Google {
    origin: Url,
    state: Arc<StubState>,
    count: watch::Receiver<usize>,
    task: JoinHandle<()>,
}

impl Google {
    async fn start() -> Self {
        let (sender, count) = watch::channel(0);
        let state = Arc::new(StubState {
            count: Some(sender),
            ..StubState::default()
        });
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("stub listener");
        let origin = Url::parse(&format!(
            "http://{}",
            listener.local_addr().expect("stub address")
        ))
        .expect("stub origin");
        let app = Router::new()
            .fallback(answer)
            .with_state(Arc::clone(&state));
        let task = tokio::spawn(async move {
            axum::serve(listener, app).await.expect("serve stub");
        });
        Self {
            origin,
            state,
            count,
            task,
        }
    }

    fn token_endpoint(&self) -> Url {
        self.origin.join("/token").expect("token endpoint")
    }

    fn answer_token(&self, answer: Answer) {
        self.state
            .token_answers
            .lock()
            .expect("answers")
            .push_back(answer);
    }

    fn answer_fcm(&self, answer: Answer) {
        self.state
            .fcm_answers
            .lock()
            .expect("answers")
            .push_back(answer);
    }

    /// Queues `answer` for the next wake to `token` alone.
    fn answer_fcm_for(&self, token: &str, answer: Answer) {
        self.state
            .fcm_answers_for
            .lock()
            .expect("answers")
            .entry(token.to_owned())
            .or_default()
            .push_back(answer);
    }

    fn requests(&self) -> Vec<Recorded> {
        self.state.requests.lock().expect("requests").clone()
    }

    fn fcm_requests(&self) -> Vec<Recorded> {
        self.requests()
            .into_iter()
            .filter(Recorded::is_fcm)
            .collect()
    }

    /// Waits until `total` requests of any kind have arrived.
    async fn wait_for(&self, total: usize) -> Vec<Recorded> {
        let mut count = self.count.clone();
        timeout(WAIT, count.wait_for(|arrived| *arrived >= total))
            .await
            .unwrap_or_else(|_| panic!("{total} requests arrive; saw {:?}", self.requests()))
            .expect("stub alive");
        self.requests()
    }

    /// Waits until `total` FCM requests have arrived.
    async fn wait_for_fcm(&self, total: usize) -> Vec<Recorded> {
        self.wait_until(&format!("{total} FCM requests arrive"), |requests| {
            requests.iter().filter(|request| request.is_fcm()).count() >= total
        })
        .await;
        self.fcm_requests()
    }

    /// Waits until the requests so far satisfy `done`.
    async fn wait_until(&self, what: &str, done: impl Fn(&[Recorded]) -> bool) -> Vec<Recorded> {
        let deadline = Instant::now() + WAIT;
        loop {
            // Read the count before the requests: a request is recorded
            // before the count moves, so none can slip between the two.
            let mut count = self.count.clone();
            let seen = *count.borrow_and_update();
            let requests = self.requests();
            if done(&requests) {
                return requests;
            }
            assert!(Instant::now() < deadline, "{what}; saw {requests:?}");
            let _ = timeout(
                deadline - Instant::now(),
                count.wait_for(|arrived| *arrived > seen),
            )
            .await;
        }
    }

    /// Holds every later wake to the FCM token `key`, or every later request
    /// to the path `key`, unanswered until the guard drops.
    async fn hold(&self, key: &str) -> tokio::sync::OwnedMutexGuard<()> {
        let gate = Arc::clone(
            self.state
                .gates
                .lock()
                .expect("gates")
                .entry(key.to_owned())
                .or_default(),
        );
        gate.lock_owned().await
    }

    /// The requests once nothing more has arrived for a quiet moment.
    async fn settled(&self) -> Vec<Recorded> {
        tokio::time::sleep(QUIET).await;
        self.requests()
    }

    fn stop(self) {
        self.task.abort();
    }
}

async fn answer(
    State(state): State<Arc<StubState>>,
    method: Method,
    uri: Uri,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let path = uri.path().to_owned();
    let recorded = Recorded {
        method: method.to_string(),
        path: path.clone(),
        headers: headers
            .iter()
            .map(|(name, value)| {
                (
                    name.as_str().to_owned(),
                    value.to_str().unwrap_or_default().to_owned(),
                )
            })
            .collect(),
        body: String::from_utf8_lossy(&body).into_owned(),
        at: Instant::now(),
    };
    let token = recorded.token();
    let key = token.clone().unwrap_or_else(|| path.clone());
    let total = {
        let mut requests = state.requests.lock().expect("requests");
        requests.push(recorded);
        requests.len()
    };
    let answer = if path == "/token" {
        state
            .token_answers
            .lock()
            .expect("answers")
            .pop_front()
            .unwrap_or(Answer {
                status: 200,
                body: json!({
                    "access_token": ACCESS_TOKEN,
                    "expires_in": 3599,
                    "token_type": "Bearer",
                }),
                retry_after: None,
            })
    } else {
        let for_token = token.as_ref().and_then(|token| {
            state
                .fcm_answers_for
                .lock()
                .expect("answers")
                .get_mut(token)
                .and_then(VecDeque::pop_front)
        });
        for_token
            .or_else(|| state.fcm_answers.lock().expect("answers").pop_front())
            .unwrap_or(Answer {
                status: 200,
                body: json!({ "name": format!("projects/{PROJECT}/messages/1") }),
                retry_after: None,
            })
    };
    if let Some(count) = &state.count {
        count.send_replace(total);
    }
    let gate = state.gates.lock().expect("gates").get(&key).cloned();
    if let Some(gate) = gate {
        drop(gate.lock().await);
    }
    let mut response = (
        StatusCode::from_u16(answer.status).expect("stub status"),
        answer.body.to_string(),
    )
        .into_response();
    if let Some(seconds) = answer.retry_after {
        response.headers_mut().insert(
            "retry-after",
            seconds.to_string().parse().expect("retry-after"),
        );
    }
    response
}

// --- The coordinator under test ---

/// The wakes owed in `database`, by the predicate `push_wake::OWED` owns.
fn owed_in(database: &Connection) -> i64 {
    database
        .query_row(
            "SELECT count(*) FROM push_wakes WHERE generation > settled",
            [],
            |row| row.get(0),
        )
        .expect("count owed wakes")
}

#[derive(Clone, Copy)]
enum Push {
    /// FCM through the stand-in, with this least gap between two wakes to
    /// one device.
    Fcm(Duration),
    Disabled,
}

struct Fixture {
    files: TempDir,
    coordinator: Coordinator,
    ws: String,
    http: String,
    stop: CancellationToken,
    task: Option<JoinHandle<Result<(), renoa_control::ControlError>>>,
    owner: PrincipalId,
    node_id: NodeId,
    task_id: TaskId,
}

fn service_account_json() -> String {
    json!({
        "type": "service_account",
        "project_id": PROJECT,
        "private_key_id": "test-key-1",
        "private_key": TEST_KEY,
        "client_email": "push@renoa-test.iam.gserviceaccount.com",
        "client_id": "1",
        "token_uri": "https://oauth2.googleapis.com/token",
    })
    .to_string()
}

fn write_service_account(directory: &Path, mode: u32) -> PathBuf {
    let path = directory.join("fcm-service-account.json");
    std::fs::write(&path, service_account_json()).expect("write the service account");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(mode))
            .expect("set the service account's mode");
    }
    #[cfg(not(unix))]
    let _ = mode;
    path
}

impl Fixture {
    async fn start(google: &Google, push: Push) -> Self {
        let files = TempDir::new().expect("temporary directory");
        let owner = PrincipalId::new();
        let node_id = NodeId::new();
        let task_id = TaskId::new();
        let coordinator = Self::open(files.path(), google, push);
        coordinator
            .create_task(TaskSpec {
                task_id,
                principal_id: owner,
                node_id,
                target: TargetRef::new("agent:scout"),
            })
            .await
            .expect("create the owner's task");
        let (ws, http, stop, task) = serve(coordinator.clone()).await;
        Self {
            files,
            coordinator,
            ws,
            http,
            stop,
            task: Some(task),
            owner,
            node_id,
            task_id,
        }
    }

    fn open(directory: &Path, google: &Google, push: Push) -> Coordinator {
        let coordinator =
            Coordinator::open_with_passkeys(directory.join("control.sqlite"), "localhost", ORIGIN)
                .expect("open the coordinator");
        match push {
            Push::Fcm(gap) => coordinator.with_push(
                PushSettings::from_service_account_file(write_service_account(directory, 0o600))
                    .expect("load the service account")
                    .with_google_endpoints(google.token_endpoint(), google.origin.clone())
                    .with_device_gap(gap),
            ),
            Push::Disabled => coordinator,
        }
    }

    /// Stops serving, then serves the same database again with a fresh
    /// coordinator, as a restart does.
    async fn restart(&mut self, google: &Google, push: Push) {
        self.restart_after(google, push, |_| {}).await;
    }

    /// Restarts after `edit` changed the database while nothing served it.
    async fn restart_after(&mut self, google: &Google, push: Push, edit: impl FnOnce(&Connection)) {
        self.stop.cancel();
        self.task
            .take()
            .expect("serving")
            .await
            .expect("coordinator task")
            .expect("coordinator stops");
        edit(&self.database());
        self.coordinator = Self::open(self.files.path(), google, push);
        let (ws, http, stop, task) = serve(self.coordinator.clone()).await;
        self.ws = ws;
        self.http = http;
        self.stop = stop;
        self.task = Some(task);
    }

    /// Stops serving and returns the coordinator's files, which are deleted
    /// when the returned directory is dropped.
    async fn stop(self) -> TempDir {
        self.stop.cancel();
        self.task
            .expect("serving")
            .await
            .expect("coordinator task")
            .expect("coordinator stops");
        self.files
    }

    fn database(&self) -> Connection {
        Connection::open(self.files.path().join("control.sqlite")).expect("open the database")
    }

    /// Every registered token, as `(device_id, principal_id, token)`.
    fn tokens(&self) -> Vec<(String, String, String)> {
        let database = self.database();
        let mut statement = database
            .prepare("SELECT device_id, principal_id, token FROM push_tokens ORDER BY token")
            .expect("query tokens");
        statement
            .query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))
            .expect("read tokens")
            .collect::<Result<_, _>>()
            .expect("tokens")
    }

    fn owed_wakes(&self) -> i64 {
        owed_in(&self.database())
    }

    /// Waits until the dispatcher has settled every owed wake.
    async fn drained(&self) {
        let deadline = Instant::now() + WAIT;
        while self.owed_wakes() > 0 {
            assert!(Instant::now() < deadline, "owed wakes drain");
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    }

    async fn enroll(&self, peer: PeerIdentity) -> DeviceCredentials {
        let token = self
            .coordinator
            .create_enrollment(peer, SystemTime::now() + Duration::from_mins(1))
            .await
            .expect("create an enrollment");
        self.claim(token).await
    }

    async fn claim(&self, token: renoa_control::EnrollmentToken) -> DeviceCredentials {
        let (mut socket, _) = connect_async(&self.ws).await.expect("connect to enroll");
        send(
            &mut socket,
            &ClientMessage::Enroll {
                version: JSON_WS_VERSION,
                token,
            },
        )
        .await;
        let ServerMessage::Enrolled { credentials, .. } = receive(&mut socket).await else {
            panic!("the coordinator enrolls the device");
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

    /// An app of `principal_id`, enrolled with its own device credential.
    async fn phone_of(&self, principal_id: PrincipalId) -> (DeviceCredentials, Socket) {
        let credentials = self
            .enroll(PeerIdentity::Surface {
                principal_id,
                surface: SurfaceRef::new("android"),
            })
            .await;
        let socket = self.connect(&credentials).await;
        (credentials, socket)
    }

    async fn phone(&self) -> (DeviceCredentials, Socket) {
        self.phone_of(self.owner).await
    }

    async fn node(&self) -> Node {
        let credentials = self
            .enroll(PeerIdentity::Node {
                node_id: self.node_id,
            })
            .await;
        Node {
            socket: self.connect(&credentials).await,
            task_id: self.task_id,
        }
    }

    /// A browser's connection, opened with a one-use ticket from a paired
    /// browser session.
    async fn browser(&self) -> Socket {
        let client = reqwest::Client::new();
        let pairing = BrowserSessions::open(self.files.path().join("control.sqlite"))
            .expect("session issuer")
            .create_pairing(self.owner, SystemTime::now() + Duration::from_mins(30))
            .await
            .expect("pairing code");
        let paired = client
            .post(format!("{}/v1/identity/pair", self.http))
            .header("origin", ORIGIN)
            .json(&json!({"pairingToken": pairing, "browserNonce": "78".repeat(32)}))
            .send()
            .await
            .expect("pair")
            .error_for_status()
            .expect("paired");
        let cookie = paired.headers()["set-cookie"]
            .to_str()
            .expect("cookie")
            .split(';')
            .next()
            .expect("cookie pair")
            .to_owned();
        let ticket: Value = client
            .post(format!("{}/v1/identity/connection-ticket", self.http))
            .header("origin", ORIGIN)
            .header("cookie", cookie)
            .json(&json!({"surface": "control_room"}))
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
}

async fn serve(
    coordinator: Coordinator,
) -> (
    String,
    String,
    CancellationToken,
    JoinHandle<Result<(), renoa_control::ControlError>>,
) {
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("listener");
    let address = listener.local_addr().expect("address");
    let stop = CancellationToken::new();
    let task = tokio::spawn(coordinator.serve(listener, stop.clone()));
    (
        format!("ws://{address}/connect"),
        format!("http://{address}"),
        stop,
        task,
    )
}

/// The node of the fixture's task, played by hand.
struct Node {
    socket: Socket,
    task_id: TaskId,
}

/// One command's execution as the node publishes it.
struct Run {
    command_id: CommandId,
    execution_id: ExecutionId,
    next: u64,
}

impl Node {
    /// Submits a command from `surface` and admits its execution.
    async fn start(&mut self, surface: &mut Socket, request_id: u64) -> Run {
        self.start_with(surface, request_id, CommandInput::from_text("what is new?"))
            .await
    }

    /// Submits `input` from `surface` and admits its execution.
    async fn start_with(
        &mut self,
        surface: &mut Socket,
        request_id: u64,
        input: CommandInput,
    ) -> Run {
        let command_id = CommandId::new();
        let quiet = input.quiet();
        send(
            surface,
            &ClientMessage::Submit {
                request_id,
                task_id: self.task_id,
                command_id,
                input,
            },
        )
        .await;
        assert_eq!(
            receive(surface).await,
            ServerMessage::CommandAccepted {
                request_id,
                command_id
            }
        );
        let ServerMessage::Execute { command, .. } = receive(&mut self.socket).await else {
            panic!("the node receives the command");
        };
        assert_eq!(command.command_id, command_id);
        assert_eq!(command.input.quiet(), quiet, "the node hears it as sent");
        send(
            &mut self.socket,
            &ClientMessage::AcknowledgeExecution {
                task_id: self.task_id,
                command_id,
            },
        )
        .await;
        assert_eq!(
            receive(&mut self.socket).await,
            ServerMessage::ExecutionAcknowledged { command_id }
        );
        Run {
            command_id,
            execution_id: ExecutionId::new(),
            next: 0,
        }
    }

    /// Publishes the next records of `run` and returns the batch as sent.
    async fn publish(
        &mut self,
        run: &mut Run,
        kinds: Vec<ExecutionEventKind>,
    ) -> Vec<ExecutionEvent> {
        let events: Vec<ExecutionEvent> = kinds
            .into_iter()
            .map(|kind| {
                let event = ExecutionEvent {
                    event_id: ExecutionEventId::new(),
                    execution_id: run.execution_id,
                    sequence: run.next,
                    recorded_at_ms: 1,
                    kind,
                };
                run.next += 1;
                event
            })
            .collect();
        self.republish(run, events.clone()).await;
        events
    }

    /// Sends a batch again, as a node does after a lost acknowledgement.
    async fn republish(&mut self, run: &Run, events: Vec<ExecutionEvent>) {
        let through = events.last().expect("a non-empty batch").sequence;
        send(
            &mut self.socket,
            &ClientMessage::PublishExecutionEvents {
                task_id: self.task_id,
                command_id: run.command_id,
                events,
            },
        )
        .await;
        assert_eq!(
            receive(&mut self.socket).await,
            ServerMessage::ExecutionEventsAccepted {
                command_id: run.command_id,
                through_execution_sequence: through,
            }
        );
    }

    /// One whole run that replies `text` and completes.
    async fn reply(&mut self, surface: &mut Socket, request_id: u64, text: &str) {
        let mut run = self.start(surface, request_id).await;
        self.publish(&mut run, replying(text)).await;
    }
}

fn replying(text: &str) -> Vec<ExecutionEventKind> {
    vec![
        ExecutionEventKind::ExecutionStarted,
        ExecutionEventKind::TurnStarted,
        ExecutionEventKind::AssistantMessage {
            text: text.to_owned(),
        },
        ExecutionEventKind::ExecutionTerminated {
            terminal: ExecutionTerminal::Completed,
        },
    ]
}

async fn send(socket: &mut Socket, message: &ClientMessage) {
    let json = serde_json::to_string(message).expect("serialize client message");
    socket
        .send(Message::Text(json.into()))
        .await
        .expect("send client message");
}

async fn send_json(socket: &mut Socket, value: &Value) {
    socket
        .send(Message::Text(value.to_string().into()))
        .await
        .expect("send frame");
}

async fn receive(socket: &mut Socket) -> ServerMessage {
    loop {
        let message = timeout(WAIT, socket.next())
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

async fn set_token(socket: &mut Socket, request_id: u64, token: &str) -> ServerMessage {
    send(
        socket,
        &ClientMessage::SetPushToken {
            request_id,
            provider: "fcm".to_owned(),
            token: PushToken::new(token),
        },
    )
    .await;
    receive(socket).await
}

fn refused(message: ServerMessage, request_id: u64, code: ErrorCode) -> String {
    match message {
        ServerMessage::Error {
            request_id: Some(answered),
            code: received,
            message,
        } if answered == request_id && received == code => message,
        other => panic!("expected {code:?} for request {request_id}, received {other:?}"),
    }
}

/// The exact body every wake carries, apart from the device's token.
fn wake_body(token: &str) -> Value {
    json!({
        "message": {
            "token": token,
            "data": { "renoa": "changed" },
            "android": { "priority": "high", "collapse_key": "changed" }
        }
    })
}

/// The exact body every ring carries, apart from the device's token.
fn ring_body(token: &str) -> Value {
    json!({
        "message": {
            "token": token,
            "data": { "renoa": "ring" },
            "android": { "priority": "high", "collapse_key": "ring" }
        }
    })
}

/// The bodies of the wakes sent so far, in order.
fn bodies(requests: &[Recorded]) -> Vec<Value> {
    requests
        .iter()
        .filter(|request| request.is_fcm())
        .map(|request| serde_json::from_str(&request.body).expect("wake JSON"))
        .collect()
}

// --- Registration ---

#[tokio::test]
async fn registering_is_idempotent_and_a_second_token_replaces_the_first() {
    let google = Google::start().await;
    let f = Fixture::start(&google, Push::Disabled).await;
    let (phone, mut socket) = f.phone().await;
    let device = phone.device_id.to_string();
    let owner = f.owner.to_string();

    for request_id in [1, 2] {
        assert_eq!(
            set_token(&mut socket, request_id, "fcm:first").await,
            ServerMessage::PushTokenSet { request_id },
            "an exact retry is acknowledged the same way"
        );
    }
    assert_eq!(
        f.tokens(),
        vec![(device.clone(), owner.clone(), "fcm:first".to_owned())]
    );
    assert_eq!(
        set_token(&mut socket, 3, "fcm:rotated").await,
        ServerMessage::PushTokenSet { request_id: 3 }
    );
    assert_eq!(
        f.tokens(),
        vec![(device, owner, "fcm:rotated".to_owned())],
        "a rotated token replaces the old one"
    );
    for request_id in [4, 5] {
        send(&mut socket, &ClientMessage::ClearPushToken { request_id }).await;
        assert_eq!(
            receive(&mut socket).await,
            ServerMessage::PushTokenCleared { request_id },
            "clearing succeeds whether or not a token is held"
        );
    }
    assert!(f.tokens().is_empty());
    f.stop().await;
    google.stop();
}

#[tokio::test]
async fn a_wrong_registration_is_refused_with_what_a_right_one_looks_like() {
    let google = Google::start().await;
    let f = Fixture::start(&google, Push::Disabled).await;
    let (_, mut phone) = f.phone().await;

    send_json(
        &mut phone,
        &json!({"type": "set_push_token", "request_id": 1, "provider": "gcm", "token": "fcm:ok"}),
    )
    .await;
    assert_eq!(
        refused(receive(&mut phone).await, 1, ErrorCode::InvalidMessage),
        "provider \"gcm\" is not supported; expected \"fcm\""
    );
    let oversize = "a".repeat(4097);
    for (request_id, token, length) in [
        (2, String::new(), 0),
        (3, oversize, 4097),
        (4, "fcm token".to_owned(), 9),
    ] {
        let message = refused(
            set_token(&mut phone, request_id, &token).await,
            request_id,
            ErrorCode::InvalidMessage,
        );
        assert_eq!(
            message,
            format!(
                "token must be 1 to 4096 bytes of printable ASCII without spaces; \
                 received {length} bytes"
            )
        );
    }

    let mut browser = f.browser().await;
    assert_eq!(
        refused(
            set_token(&mut browser, 5, "fcm:browser").await,
            5,
            ErrorCode::InvalidRole
        ),
        "push tokens belong to an enrolled device; this connection was opened with a browser ticket"
    );
    send(
        &mut browser,
        &ClientMessage::ClearPushToken { request_id: 6 },
    )
    .await;
    refused(receive(&mut browser).await, 6, ErrorCode::InvalidRole);

    let mut node = f.node().await;
    assert_eq!(
        refused(
            set_token(&mut node.socket, 7, "fcm:node").await,
            7,
            ErrorCode::InvalidRole
        ),
        "node cannot issue surface operations"
    );
    assert!(f.tokens().is_empty(), "no refused registration wrote a row");
    f.stop().await;
    google.stop();
}

#[tokio::test]
async fn a_token_belongs_to_one_principal_and_moves_between_its_devices() {
    let google = Google::start().await;
    let f = Fixture::start(&google, Push::Disabled).await;
    let (first, mut first_socket) = f.phone().await;
    let (stranger, mut stranger_socket) = f.phone_of(PrincipalId::new()).await;
    let (second, mut second_socket) = f.phone().await;
    let owner = f.owner.to_string();

    assert!(matches!(
        set_token(&mut first_socket, 1, "fcm:shared").await,
        ServerMessage::PushTokenSet { .. }
    ));
    assert_eq!(
        refused(
            set_token(&mut stranger_socket, 2, "fcm:shared").await,
            2,
            ErrorCode::Conflict
        ),
        "token is registered to another principal; sign that device out first"
    );
    assert_eq!(
        f.tokens(),
        vec![(
            first.device_id.to_string(),
            owner.clone(),
            "fcm:shared".to_owned()
        )],
        "the first registration is intact"
    );
    assert_ne!(stranger.device_id, first.device_id);

    assert!(matches!(
        set_token(&mut second_socket, 3, "fcm:shared").await,
        ServerMessage::PushTokenSet { .. }
    ));
    assert_eq!(
        f.tokens(),
        vec![(second.device_id.to_string(), owner, "fcm:shared".to_owned())],
        "the token moved to the owner's other device"
    );
    f.stop().await;
    google.stop();
}

// --- Waking ---

#[tokio::test]
async fn a_reply_wakes_the_phone_with_a_constant_request_that_carries_no_content() {
    let google = Google::start().await;
    let f = Fixture::start(&google, Push::Fcm(Duration::ZERO)).await;
    let (_, mut phone) = f.phone().await;
    assert!(matches!(
        set_token(&mut phone, 1, "fcm:phone-token").await,
        ServerMessage::PushTokenSet { .. }
    ));
    let mut node = f.node().await;
    let distinctive = "Zanzibar-7f3c: the tide tables changed";

    let mut run = node.start(&mut phone, 2).await;
    let batch = node.publish(&mut run, replying(distinctive)).await;
    let requests = google.wait_for(2).await;

    let token_request = &requests[0];
    assert_eq!(token_request.path, "/token");
    let form: Vec<(String, String)> = url::form_urlencoded::parse(token_request.body.as_bytes())
        .into_owned()
        .collect();
    assert_eq!(
        form[0],
        (
            "grant_type".to_owned(),
            "urn:ietf:params:oauth:grant-type:jwt-bearer".to_owned()
        )
    );
    let claims: Value = serde_json::from_slice(
        &URL_SAFE_NO_PAD
            .decode(form[1].1.split('.').nth(1).expect("claims"))
            .expect("base64url claims"),
    )
    .expect("claims JSON");
    assert_eq!(claims["aud"], google.token_endpoint().as_str());
    assert_eq!(
        claims["scope"],
        "https://www.googleapis.com/auth/firebase.messaging"
    );

    let wake = &requests[1];
    assert_eq!(wake.method, "POST");
    assert_eq!(wake.path, format!("/v1/projects/{PROJECT}/messages:send"));
    assert_eq!(
        wake.header("authorization"),
        Some(format!("Bearer {ACCESS_TOKEN}").as_str())
    );
    assert_eq!(
        serde_json::from_str::<Value>(&wake.body).expect("wake JSON"),
        wake_body("fcm:phone-token"),
        "the body is the constant payload"
    );
    for request in &requests {
        let everything = request.everything();
        for absent in [
            distinctive.to_owned(),
            "Zanzibar".to_owned(),
            f.task_id.to_string(),
            f.owner.to_string(),
        ] {
            assert!(!everything.contains(&absent), "{absent} in {everything}");
        }
    }

    // The node did not hear the acknowledgement and sends the batch again:
    // nothing new is appended, so nothing new wakes.
    node.republish(&run, batch).await;
    assert_eq!(
        google.settled().await.len(),
        2,
        "a retried batch wakes nothing"
    );
    assert_eq!(f.owed_wakes(), 0);
    f.stop().await;
    google.stop();
}

#[tokio::test]
async fn a_failed_run_wakes_the_phone_and_a_quiet_or_cancelled_run_does_not() {
    let google = Google::start().await;
    let f = Fixture::start(&google, Push::Fcm(Duration::ZERO)).await;
    let (_, mut phone) = f.phone().await;
    set_token(&mut phone, 1, "fcm:phone-token").await;
    let mut node = f.node().await;

    let mut quiet = node.start(&mut phone, 2).await;
    node.publish(
        &mut quiet,
        vec![
            ExecutionEventKind::ExecutionStarted,
            ExecutionEventKind::TurnStarted,
            ExecutionEventKind::ToolStarted {
                call_id: "call-1".to_owned(),
                name: "search".to_owned(),
                arguments: json!({}),
            },
            ExecutionEventKind::ToolFinished {
                call_id: "call-1".to_owned(),
                output: "nothing".to_owned(),
                is_error: false,
            },
            ExecutionEventKind::AssistantMessage {
                text: String::new(),
            },
            ExecutionEventKind::AssistantMessage {
                text: " \n\t".to_owned(),
            },
            ExecutionEventKind::ExecutionTerminated {
                terminal: ExecutionTerminal::Completed,
            },
        ],
    )
    .await;
    let mut cancelled = node.start(&mut phone, 3).await;
    node.publish(
        &mut cancelled,
        vec![
            ExecutionEventKind::ExecutionStarted,
            ExecutionEventKind::ExecutionTerminated {
                terminal: ExecutionTerminal::Cancelled {
                    reason: "stopped".to_owned(),
                },
            },
        ],
    )
    .await;
    assert!(
        google.settled().await.is_empty(),
        "no record of theirs wakes"
    );
    assert_eq!(f.owed_wakes(), 0, "and none is owed");

    let mut failed = node.start(&mut phone, 4).await;
    node.publish(
        &mut failed,
        vec![
            ExecutionEventKind::ExecutionStarted,
            ExecutionEventKind::ExecutionTerminated {
                terminal: ExecutionTerminal::Failed {
                    error: "the model is unavailable".to_owned(),
                },
            },
        ],
    )
    .await;
    let fcm = google.wait_for_fcm(1).await;
    assert_eq!(
        serde_json::from_str::<Value>(&fcm[0].body).expect("wake JSON"),
        wake_body("fcm:phone-token")
    );
    f.stop().await;
    google.stop();
}

/// A heartbeat's run is submitted quiet: neither its text nor its failure
/// wakes the phone, and the owner's own command on the same task still does.
#[tokio::test]
async fn the_records_of_a_quiet_command_wake_no_one() {
    let google = Google::start().await;
    let f = Fixture::start(&google, Push::Fcm(Duration::ZERO)).await;
    let (_, mut phone) = f.phone().await;
    set_token(&mut phone, 1, "fcm:phone-token").await;
    let mut node = f.node().await;
    let quiet = || CommandInput::Text {
        text: "This is your heartbeat.".to_owned(),
        context: None,
        author: Author::Principal,
        quiet: true,
        attachments: Vec::new(),
    };

    let mut heartbeat = node.start_with(&mut phone, 2, quiet()).await;
    node.publish(&mut heartbeat, replying("Nothing to report."))
        .await;
    let mut failed = node.start_with(&mut phone, 3, quiet()).await;
    node.publish(
        &mut failed,
        vec![
            ExecutionEventKind::ExecutionStarted,
            ExecutionEventKind::ExecutionTerminated {
                terminal: ExecutionTerminal::Failed {
                    error: "the model is unavailable".to_owned(),
                },
            },
        ],
    )
    .await;
    assert!(google.settled().await.is_empty(), "nothing of theirs wakes");
    assert_eq!(f.owed_wakes(), 0, "and none is owed");

    let mut owners = node.start(&mut phone, 4).await;
    node.publish(&mut owners, replying("Here is what changed."))
        .await;
    let fcm = google.wait_for_fcm(1).await;
    assert_eq!(
        serde_json::from_str::<Value>(&fcm[0].body).expect("wake JSON"),
        wake_body("fcm:phone-token")
    );
    f.stop().await;
    google.stop();
}

#[tokio::test]
async fn a_burst_sends_one_wake_now_and_one_trailing_wake_after_the_gap() {
    let gap = Duration::from_millis(1500);
    let google = Google::start().await;
    let f = Fixture::start(&google, Push::Fcm(gap)).await;
    let (_, mut phone) = f.phone().await;
    set_token(&mut phone, 1, "fcm:phone-token").await;
    let mut node = f.node().await;

    let mut run = node.start(&mut phone, 2).await;
    node.publish(
        &mut run,
        vec![
            ExecutionEventKind::ExecutionStarted,
            ExecutionEventKind::TurnStarted,
            ExecutionEventKind::AssistantMessage {
                text: "one".to_owned(),
            },
        ],
    )
    .await;
    google.wait_for_fcm(1).await;
    for text in ["two", "three", "four", "five"] {
        node.publish(
            &mut run,
            vec![ExecutionEventKind::AssistantMessage {
                text: text.to_owned(),
            }],
        )
        .await;
    }
    let fcm = google.wait_for_fcm(2).await;
    assert!(
        fcm[1].at - fcm[0].at >= gap,
        "the trailing wake waits out the gap: {:?}",
        fcm[1].at - fcm[0].at
    );
    tokio::time::sleep(gap).await;
    assert_eq!(
        google.settled().await.iter().filter(|r| r.is_fcm()).count(),
        2,
        "a burst is one wake and one trailing wake"
    );
    assert_eq!(f.owed_wakes(), 0);
    f.stop().await;
    google.stop();
}

#[tokio::test]
async fn an_owed_wake_survives_a_restart_and_is_sent_once() {
    let push = Push::Fcm(Duration::from_mins(1));
    let google = Google::start().await;
    let mut f = Fixture::start(&google, push).await;
    let (phone, mut socket) = f.phone().await;
    set_token(&mut socket, 1, "fcm:phone-token").await;
    let mut node = f.node().await;
    node.reply(&mut socket, 2, "first").await;
    google.wait_for_fcm(1).await;
    // Inside the gap: the wake is committed and owed, not sent.
    node.reply(&mut socket, 3, "second").await;
    assert_eq!(f.owed_wakes(), 1);
    assert_eq!(
        google.settled().await.iter().filter(|r| r.is_fcm()).count(),
        1
    );
    drop((socket, node));

    f.restart(&google, push).await;
    google.wait_for_fcm(2).await;
    f.drained().await;
    assert_eq!(
        google.settled().await.iter().filter(|r| r.is_fcm()).count(),
        2,
        "exactly one wake after the restart"
    );
    assert_eq!(f.tokens().len(), 1);
    assert_eq!(f.tokens()[0].0, phone.device_id.to_string());
    f.stop().await;
    google.stop();
}

#[tokio::test]
async fn a_wake_recorded_during_its_send_is_sent_again() {
    let google = Google::start().await;
    let f = Fixture::start(&google, Push::Fcm(Duration::ZERO)).await;
    let (_, mut phone) = f.phone().await;
    set_token(&mut phone, 1, "fcm:phone-token").await;
    let mut node = f.node().await;
    let held = google.hold("fcm:phone-token").await;
    node.reply(&mut phone, 2, "first").await;
    google.wait_for_fcm(1).await;
    // Recorded after the first send started, so that send does not cover it.
    node.reply(&mut phone, 3, "second").await;
    drop(held);
    google.wait_for_fcm(2).await;
    f.drained().await;
    assert_eq!(
        google.settled().await.iter().filter(|r| r.is_fcm()).count(),
        2
    );
    f.stop().await;
    google.stop();
}

#[tokio::test]
async fn a_device_marked_revoked_is_not_woken_though_its_token_row_remains() {
    let google = Google::start().await;
    let f = Fixture::start(&google, Push::Fcm(Duration::ZERO)).await;
    let (phone, mut phone_socket) = f.phone().await;
    set_token(&mut phone_socket, 1, "fcm:phone-token").await;
    let (_, mut laptop) = f.phone().await;
    let mut node = f.node().await;
    f.database()
        .execute(
            "UPDATE devices SET revoked = 1 WHERE device_id = ?1",
            [phone.device_id.to_string()],
        )
        .expect("mark the phone revoked");

    node.reply(&mut laptop, 2, "news").await;
    f.drained().await;
    assert!(
        google.settled().await.is_empty(),
        "a revoked device is not woken"
    );
    assert_eq!(f.tokens().len(), 1, "the token row is still there");
    f.stop().await;
    google.stop();
}

#[tokio::test]
async fn a_token_rotated_during_a_send_survives_a_late_unregistered() {
    let google = Google::start().await;
    let f = Fixture::start(&google, Push::Fcm(Duration::ZERO)).await;
    let (phone, mut socket) = f.phone().await;
    set_token(&mut socket, 1, "fcm:old").await;
    let mut node = f.node().await;
    google.answer_fcm(Answer::fcm_error(404, "UNREGISTERED", "NOT_FOUND"));
    let held = google.hold("fcm:old").await;
    node.reply(&mut socket, 2, "news").await;
    google.wait_for_fcm(1).await;
    assert_eq!(
        set_token(&mut socket, 3, "fcm:new").await,
        ServerMessage::PushTokenSet { request_id: 3 }
    );
    drop(held);
    f.drained().await;
    assert_eq!(
        f.tokens(),
        vec![(
            phone.device_id.to_string(),
            f.owner.to_string(),
            "fcm:new".to_owned()
        )],
        "only the token FCM called dead is deleted"
    );
    f.stop().await;
    google.stop();
}

#[tokio::test]
async fn a_token_rotated_during_a_delivered_send_is_woken_too() {
    let google = Google::start().await;
    let f = Fixture::start(&google, Push::Fcm(Duration::ZERO)).await;
    let (_, mut socket) = f.phone().await;
    set_token(&mut socket, 1, "fcm:old").await;
    let mut node = f.node().await;
    let held = google.hold("fcm:old").await;
    node.reply(&mut socket, 2, "news").await;
    google.wait_for_fcm(1).await;
    assert_eq!(
        set_token(&mut socket, 3, "fcm:new").await,
        ServerMessage::PushTokenSet { request_id: 3 }
    );
    // The old token is delivered, but the phone no longer answers to it.
    drop(held);
    google
        .wait_until("the new token is sent the wake", |requests| {
            wakes_to(requests, "fcm:new") == 1
        })
        .await;
    f.drained().await;
    let requests = google.settled().await;
    assert_eq!(wakes_to(&requests, "fcm:old"), 1, "{requests:?}");
    assert_eq!(wakes_to(&requests, "fcm:new"), 1, "{requests:?}");
    f.stop().await;
    google.stop();
}

#[tokio::test]
async fn a_send_for_an_old_wake_does_not_cover_a_new_one() {
    let google = Google::start().await;
    let f = Fixture::start(&google, Push::Fcm(Duration::ZERO)).await;
    let (_, mut phone) = f.phone().await;
    set_token(&mut phone, 1, "fcm:phone").await;
    // A device with no token, through which the owner talks.
    let (_, mut laptop) = f.phone().await;
    let mut node = f.node().await;
    let held = google.hold("fcm:phone").await;
    node.reply(&mut laptop, 2, "first").await;
    google.wait_for_fcm(1).await;

    // While that send is in flight, the phone leaves, the first wake settles
    // with no device to wake, and the phone comes back with the same token.
    send(&mut phone, &ClientMessage::ClearPushToken { request_id: 3 }).await;
    assert_eq!(
        receive(&mut phone).await,
        ServerMessage::PushTokenCleared { request_id: 3 }
    );
    node.reply(&mut laptop, 4, "second").await;
    f.drained().await;
    assert_eq!(
        set_token(&mut phone, 5, "fcm:phone").await,
        ServerMessage::PushTokenSet { request_id: 5 }
    );
    node.reply(&mut laptop, 6, "third").await;
    assert_eq!(f.owed_wakes(), 1);

    // The old send ends now: it started before the third wake, so it covers
    // nothing the phone is owed.
    drop(held);
    google
        .wait_until("the phone is sent the third wake", |requests| {
            wakes_to(requests, "fcm:phone") == 2
        })
        .await;
    f.drained().await;
    f.stop().await;
    google.stop();
}

#[tokio::test]
async fn a_device_that_had_the_wake_is_not_woken_again_while_another_waits_out_its_gap() {
    let gap = Duration::from_millis(1500);
    let google = Google::start().await;
    let f = Fixture::start(&google, Push::Fcm(gap)).await;
    let (_, mut phone) = f.phone().await;
    set_token(&mut phone, 1, "fcm:phone").await;
    let mut node = f.node().await;
    node.reply(&mut phone, 2, "first").await;
    google.wait_for_fcm(1).await;
    let (_, mut tablet) = f.phone().await;
    set_token(&mut tablet, 1, "fcm:tablet").await;

    node.reply(&mut phone, 3, "second").await;
    google
        .wait_until("the tablet, outside any gap, is woken now", |requests| {
            wakes_to(requests, "fcm:tablet") == 1
        })
        .await;
    // The phone's trailing wake goes out after its gap and is held past the
    // tablet's gap too: nothing may wake the tablet for this wake again.
    let held = google.hold("fcm:phone").await;
    google
        .wait_until("the phone's trailing wake", |requests| {
            wakes_to(requests, "fcm:phone") == 2
        })
        .await;
    tokio::time::sleep(gap + QUIET).await;
    drop(held);
    f.drained().await;
    let requests = google.settled().await;
    assert_eq!(wakes_to(&requests, "fcm:tablet"), 1, "{requests:?}");
    assert_eq!(wakes_to(&requests, "fcm:phone"), 2, "{requests:?}");
    f.stop().await;
    google.stop();
}

#[tokio::test]
async fn a_device_whose_send_hangs_holds_up_no_other_device() {
    let google = Google::start().await;
    let f = Fixture::start(&google, Push::Fcm(Duration::ZERO)).await;
    let (_, mut phone) = f.phone().await;
    set_token(&mut phone, 1, "fcm:stuck").await;
    let (_, mut tablet) = f.phone().await;
    set_token(&mut tablet, 1, "fcm:free").await;
    let mut node = f.node().await;
    let held = google.hold("fcm:stuck").await;

    node.reply(&mut phone, 2, "first").await;
    google
        .wait_until("both devices are sent the first wake", |requests| {
            wakes_to(requests, "fcm:stuck") == 1 && wakes_to(requests, "fcm:free") == 1
        })
        .await;
    node.reply(&mut phone, 3, "second").await;
    google
        .wait_until(
            "the free device gets the second wake while the other hangs",
            |requests| wakes_to(requests, "fcm:free") == 2,
        )
        .await;
    drop(held);
    google
        .wait_until("the hung device gets the second wake after", |requests| {
            wakes_to(requests, "fcm:stuck") == 2
        })
        .await;
    f.drained().await;
    f.stop().await;
    google.stop();
}

#[tokio::test]
async fn at_most_eight_wakes_are_in_flight_and_a_finished_one_starts_the_next() {
    let google = Google::start().await;
    let f = Fixture::start(&google, Push::Fcm(Duration::ZERO)).await;
    let mut phones = Vec::new();
    let mut held = HashMap::new();
    for index in 0..9 {
        let (_, mut phone) = f.phone().await;
        let token = format!("fcm:device-{index}");
        set_token(&mut phone, 1, &token).await;
        held.insert(token.clone(), google.hold(&token).await);
        phones.push(phone);
    }
    let mut node = f.node().await;

    node.reply(&mut phones[0], 2, "news").await;
    google.wait_for_fcm(8).await;
    let first: Vec<Recorded> = google
        .settled()
        .await
        .into_iter()
        .filter(Recorded::is_fcm)
        .collect();
    assert_eq!(first.len(), 8, "no more than eight sends at once");
    let finished = first[0].token().expect("a wake names its token");
    drop(held.remove(&finished));
    let fcm = google.wait_for_fcm(9).await;
    let ninth = fcm[8].token().expect("a wake names its token");
    assert!(
        first
            .iter()
            .all(|request| request.token().as_deref() != Some(ninth.as_str())),
        "the finished send makes room for the device that waited"
    );
    drop(held);
    f.drained().await;
    assert_eq!(
        google.settled().await.iter().filter(|r| r.is_fcm()).count(),
        9
    );
    f.stop().await;
    google.stop();
}

#[tokio::test]
async fn a_storage_failure_is_retried_and_the_owed_wake_still_goes_out() {
    let google = Google::start().await;
    let f = Fixture::start(&google, Push::Fcm(Duration::ZERO)).await;
    let (_, mut phone) = f.phone().await;
    set_token(&mut phone, 1, "fcm:phone-token").await;
    let mut node = f.node().await;
    node.reply(&mut phone, 2, "first").await;
    google.wait_for_fcm(1).await;
    f.drained().await;

    let database = f.database();
    database
        .execute(
            "INSERT INTO push_wakes (principal_id, generation) VALUES ('not-a-principal', 1)",
            [],
        )
        .expect("plant a marker the dispatcher cannot read");
    node.reply(&mut phone, 3, "second").await;
    // Its pass fails on the planted marker. Removing it tells nobody: only
    // the dispatcher's retry finds the owed wake.
    tokio::time::sleep(QUIET).await;
    database
        .execute(
            "DELETE FROM push_wakes WHERE principal_id = 'not-a-principal'",
            [],
        )
        .expect("remove the planted marker");
    google.wait_for_fcm(2).await;
    f.drained().await;
    f.stop().await;
    google.stop();
}

#[tokio::test]
async fn a_shutdown_mid_send_keeps_the_wake_owed_and_the_next_serve_sends_it() {
    let push = Push::Fcm(Duration::ZERO);
    let google = Google::start().await;
    let mut f = Fixture::start(&google, push).await;
    let (_, mut socket) = f.phone().await;
    set_token(&mut socket, 1, "fcm:phone-token").await;
    let mut node = f.node().await;
    let held = google.hold("fcm:phone-token").await;
    node.reply(&mut socket, 2, "news").await;
    google.wait_for_fcm(1).await;
    drop((socket, node));

    f.restart_after(&google, push, |database| {
        assert_eq!(
            owed_in(database),
            1,
            "a send cut short by shutdown leaves the wake owed"
        );
    })
    .await;
    google.wait_for_fcm(2).await;
    drop(held);
    f.drained().await;
    f.stop().await;
    google.stop();
}

#[tokio::test]
async fn dropping_serve_stops_its_dispatcher() {
    let gap = Duration::from_millis(1500);
    let google = Google::start().await;
    let mut f = Fixture::start(&google, Push::Fcm(gap)).await;
    let (_, mut phone) = f.phone().await;
    set_token(&mut phone, 1, "fcm:phone-token").await;
    let mut node = f.node().await;
    node.reply(&mut phone, 2, "first").await;
    google.wait_for_fcm(1).await;
    // Inside the gap: owed, and due once the gap has passed.
    node.reply(&mut phone, 3, "second").await;
    assert_eq!(f.owed_wakes(), 1);

    let serving = f.task.take().expect("serving");
    serving.abort();
    let _ = serving.await;
    tokio::time::sleep(gap + QUIET).await;
    assert_eq!(
        google.requests().iter().filter(|r| r.is_fcm()).count(),
        1,
        "no dispatcher outlives serve"
    );
    assert_eq!(f.owed_wakes(), 1);
    google.stop();
}

// --- FCM's answers ---

#[tokio::test]
async fn a_dead_token_deletes_its_row() {
    let google = Google::start().await;
    let f = Fixture::start(&google, Push::Fcm(Duration::ZERO)).await;
    let (_, mut phone) = f.phone().await;
    let mut node = f.node().await;
    for (index, answer) in [
        Answer::fcm_error(404, "UNREGISTERED", "NOT_FOUND"),
        Answer::invalid_argument_on_the_token(),
        Answer::token_violation(),
    ]
    .into_iter()
    .enumerate()
    {
        let request_id = u64::try_from(index).expect("index") * 10;
        set_token(&mut phone, request_id + 1, "fcm:phone-token").await;
        assert_eq!(f.tokens().len(), 1);
        google.answer_fcm(answer);
        node.reply(&mut phone, request_id + 2, "news").await;
        google.wait_for_fcm(index + 1).await;
        f.drained().await;
        assert!(f.tokens().is_empty(), "the dead token's row is deleted");
    }
    assert_eq!(
        google.settled().await.iter().filter(|r| r.is_fcm()).count(),
        3,
        "a dead token is not retried"
    );
    f.stop().await;
    google.stop();
}

#[tokio::test]
async fn an_outage_longer_than_the_retries_is_waited_out_and_the_wake_still_goes_out() {
    let google = Google::start().await;
    let f = Fixture::start(&google, Push::Fcm(Duration::ZERO)).await;
    let (_, mut phone) = f.phone().await;
    set_token(&mut phone, 1, "fcm:phone-token").await;
    let mut node = f.node().await;
    for _ in 0..3 {
        let mut answer = Answer::fcm_error(503, "UNAVAILABLE", "UNAVAILABLE");
        // Longer than the first backoff step, so ignoring it shows.
        answer.retry_after = Some(2);
        google.answer_fcm(answer);
    }
    node.reply(&mut phone, 2, "news").await;
    // Three attempts give up; the device stays owed and FCM has recovered
    // by the next send.
    let fcm = google.wait_for_fcm(4).await;
    for pair in fcm[..3].windows(2) {
        assert!(
            pair[1].at - pair[0].at >= Duration::from_secs(2),
            "each retry waits out Retry-After"
        );
    }
    assert!(
        fcm[3].at - fcm[2].at >= Duration::from_secs(2),
        "the device's backoff passes before it is sent again"
    );
    f.drained().await;
    assert_eq!(
        google.settled().await.iter().filter(|r| r.is_fcm()).count(),
        4,
        "the delivered send settles the wake"
    );
    assert_eq!(f.tokens().len(), 1, "an outage deletes nothing");
    f.stop().await;
    google.stop();
}

#[tokio::test]
async fn a_configuration_fault_deletes_nothing_and_is_not_retried() {
    let google = Google::start().await;
    let f = Fixture::start(&google, Push::Fcm(Duration::ZERO)).await;
    let (_, mut phone) = f.phone().await;
    set_token(&mut phone, 1, "fcm:phone-token").await;
    let mut node = f.node().await;
    google.answer_fcm(Answer::fcm_error(
        403,
        "SENDER_ID_MISMATCH",
        "PERMISSION_DENIED",
    ));
    node.reply(&mut phone, 2, "more news").await;
    google.wait_for_fcm(1).await;
    f.drained().await;
    let requests = google.settled().await;
    assert_eq!(
        requests.iter().filter(|r| r.is_fcm()).count(),
        1,
        "a sender mismatch is not retried"
    );
    assert_eq!(f.tokens().len(), 1, "a configuration fault deletes nothing");
    f.stop().await;
    google.stop();
}

#[tokio::test]
async fn a_rejected_service_account_is_asked_once_for_every_device_and_wake() {
    let google = Google::start().await;
    let f = Fixture::start(&google, Push::Fcm(Duration::ZERO)).await;
    let (_, mut phone) = f.phone().await;
    set_token(&mut phone, 1, "fcm:phone").await;
    let (_, mut tablet) = f.phone().await;
    set_token(&mut tablet, 1, "fcm:tablet").await;
    let mut node = f.node().await;
    google.answer_token(Answer {
        status: 401,
        body: json!({"error": "invalid_client"}),
        retry_after: None,
    });
    node.reply(&mut phone, 2, "news").await;
    google.wait_for(1).await;
    node.reply(&mut phone, 3, "more news").await;
    // Past both devices' first backoff, inside the rejection's hold.
    tokio::time::sleep(Duration::from_secs(3)).await;
    let requests = google.settled().await;
    assert_eq!(
        requests.len(),
        1,
        "one rejection answers both devices, their retries and the next wake: {requests:?}"
    );
    assert_eq!(f.owed_wakes(), 1, "the wake stays owed");
    f.stop().await;
    google.stop();
}

#[tokio::test]
async fn a_failing_token_endpoint_leaves_the_wake_owed_until_it_answers() {
    let push = Push::Fcm(Duration::ZERO);
    let google = Google::start().await;
    let mut f = Fixture::start(&google, push).await;
    let (_, mut phone) = f.phone().await;
    set_token(&mut phone, 1, "fcm:phone-token").await;
    let mut node = f.node().await;

    // A rejection says nothing about the device, which stays owed.
    google.answer_token(Answer {
        status: 400,
        body: json!({"error": "invalid_grant", "error_description": "Invalid JWT Signature."}),
        retry_after: None,
    });
    node.reply(&mut phone, 2, "news").await;
    google.wait_for(1).await;
    let requests = google.settled().await;
    assert_eq!(requests.len(), 1, "{requests:?}");
    assert_eq!(f.owed_wakes(), 1, "a rejected sign-in keeps the wake owed");
    assert_eq!(f.tokens().len(), 1, "a rejected sign-in deletes nothing");
    drop((phone, node));

    // A rejection is held for a minute; a restart forgets it. The next
    // answer is no token at all, which is not held: the device's backoff
    // passes and the endpoint, recovered, grants one.
    google.answer_token(Answer {
        status: 200,
        body: json!({"not": "a token"}),
        retry_after: None,
    });
    f.restart(&google, push).await;
    google.wait_for_fcm(1).await;
    f.drained().await;
    let requests = google.settled().await;
    let paths: Vec<&str> = requests[1..].iter().map(|r| r.path.as_str()).collect();
    assert_eq!(
        paths,
        ["/token", "/token", "/v1/projects/renoa-test/messages:send"]
    );
    assert!(
        requests[2].at - requests[1].at >= Duration::from_secs(2),
        "the device's backoff passes before it asks again"
    );
    f.stop().await;
    google.stop();
}

#[tokio::test]
async fn a_bare_bad_request_deletes_nothing_and_is_not_retried() {
    let google = Google::start().await;
    let f = Fixture::start(&google, Push::Fcm(Duration::ZERO)).await;
    let (_, mut phone) = f.phone().await;
    set_token(&mut phone, 1, "fcm:phone-token").await;
    let mut node = f.node().await;
    google.answer_fcm(Answer {
        status: 400,
        body: json!({"error": {"code": 400, "status": "INVALID_ARGUMENT", "message": "stub"}}),
        retry_after: None,
    });
    node.reply(&mut phone, 2, "news").await;
    google.wait_for_fcm(1).await;
    f.drained().await;
    assert_eq!(
        google.settled().await.iter().filter(|r| r.is_fcm()).count(),
        1
    );
    assert_eq!(f.tokens().len(), 1, "no detail named the token");
    f.stop().await;
    google.stop();
}

#[tokio::test]
async fn a_gateway_failure_or_a_throttle_is_retried() {
    let google = Google::start().await;
    let f = Fixture::start(&google, Push::Fcm(Duration::ZERO)).await;
    let (_, mut phone) = f.phone().await;
    set_token(&mut phone, 1, "fcm:phone-token").await;
    let mut node = f.node().await;
    google.answer_fcm(Answer::status(502));
    google.answer_fcm(Answer::status(429));
    node.reply(&mut phone, 2, "news").await;
    google.wait_for_fcm(3).await;
    f.drained().await;
    assert_eq!(
        google.settled().await.iter().filter(|r| r.is_fcm()).count(),
        3,
        "two retries, then the default answer delivers"
    );
    assert_eq!(f.tokens().len(), 1);
    f.stop().await;
    google.stop();
}

#[tokio::test]
async fn a_busy_token_endpoint_is_retried() {
    let google = Google::start().await;
    let f = Fixture::start(&google, Push::Fcm(Duration::ZERO)).await;
    let (_, mut phone) = f.phone().await;
    set_token(&mut phone, 1, "fcm:phone-token").await;
    let mut node = f.node().await;
    google.answer_token(Answer::status(429));
    google.answer_token(Answer::status(408));
    node.reply(&mut phone, 2, "news").await;
    google.wait_for_fcm(1).await;
    f.drained().await;
    let requests = google.settled().await;
    let paths: Vec<&str> = requests.iter().map(|r| r.path.as_str()).collect();
    assert_eq!(
        paths,
        [
            "/token",
            "/token",
            "/token",
            "/v1/projects/renoa-test/messages:send"
        ]
    );
    f.stop().await;
    google.stop();
}

/// A token endpoint's answer granting `token`.
fn access_token(token: &str) -> Answer {
    Answer {
        status: 200,
        body: json!({"access_token": token, "expires_in": 3599, "token_type": "Bearer"}),
        retry_after: None,
    }
}

/// The access token each FCM request in `requests` was sent with.
fn bearers(requests: &[Recorded]) -> Vec<&str> {
    requests
        .iter()
        .filter(|request| request.is_fcm())
        .map(|request| request.header("authorization").expect("an authorization"))
        .collect()
}

#[tokio::test]
async fn a_refused_access_token_is_fetched_afresh_once_per_wake() {
    let send = "/v1/projects/renoa-test/messages:send";
    let google = Google::start().await;
    let f = Fixture::start(&google, Push::Fcm(Duration::ZERO)).await;
    let (_, mut phone) = f.phone().await;
    set_token(&mut phone, 1, "fcm:phone-token").await;
    let mut node = f.node().await;
    google.answer_token(access_token("ya29.first"));
    google.answer_token(access_token("ya29.second"));
    google.answer_fcm(Answer::status(401));
    node.reply(&mut phone, 2, "news").await;
    google.wait_for_fcm(2).await;
    f.drained().await;
    let requests = google.settled().await;
    let paths: Vec<&str> = requests.iter().map(|r| r.path.as_str()).collect();
    assert_eq!(paths, ["/token", send, "/token", send]);
    assert_eq!(
        bearers(&requests),
        ["Bearer ya29.first", "Bearer ya29.second"]
    );

    // A fresh token refused too is a configuration fault, not retried.
    google.answer_token(access_token("ya29.third"));
    google.answer_fcm(Answer::status(401));
    google.answer_fcm(Answer::status(401));
    node.reply(&mut phone, 3, "more news").await;
    google.wait_for_fcm(4).await;
    f.drained().await;
    let requests = google.settled().await;
    let paths: Vec<&str> = requests[4..].iter().map(|r| r.path.as_str()).collect();
    assert_eq!(paths, [send, "/token", send]);
    assert_eq!(
        bearers(&requests[4..]),
        ["Bearer ya29.second", "Bearer ya29.third"]
    );
    assert_eq!(
        f.tokens().len(),
        1,
        "a refused access token deletes nothing"
    );
    f.stop().await;
    google.stop();
}

#[tokio::test]
async fn a_late_refusal_of_an_old_access_token_keeps_the_new_one() {
    let google = Google::start().await;
    let f = Fixture::start(&google, Push::Fcm(Duration::ZERO)).await;
    let (_, mut phone) = f.phone().await;
    set_token(&mut phone, 1, "fcm:phone").await;
    let (_, mut tablet) = f.phone().await;
    set_token(&mut tablet, 1, "fcm:tablet").await;
    let mut node = f.node().await;
    google.answer_token(access_token("ya29.first"));
    google.answer_token(access_token("ya29.second"));
    google.answer_fcm_for("fcm:phone", Answer::status(401));
    google.answer_fcm_for("fcm:tablet", Answer::status(401));
    let held = google.hold("fcm:tablet").await;

    node.reply(&mut phone, 2, "news").await;
    google
        .wait_until("the phone is sent again with a fresh token", |requests| {
            wakes_to(requests, "fcm:phone") == 2 && wakes_to(requests, "fcm:tablet") == 1
        })
        .await;
    // The tablet's refusal of the first token arrives after the second
    // replaced it.
    drop(held);
    google
        .wait_until("the tablet is sent again", |requests| {
            wakes_to(requests, "fcm:tablet") == 2
        })
        .await;
    f.drained().await;
    let requests = google.settled().await;
    assert_eq!(
        requests.iter().filter(|r| r.path == "/token").count(),
        2,
        "the late refusal does not discard the fresh token: {requests:?}"
    );
    let tablet: Vec<Recorded> = requests
        .iter()
        .filter(|r| r.token().as_deref() == Some("fcm:tablet"))
        .cloned()
        .collect();
    assert_eq!(
        bearers(&tablet),
        ["Bearer ya29.first", "Bearer ya29.second"]
    );
    f.stop().await;
    google.stop();
}

#[tokio::test]
async fn sends_waiting_on_a_failed_token_fetch_share_its_failure() {
    let google = Google::start().await;
    let f = Fixture::start(&google, Push::Fcm(Duration::ZERO)).await;
    let mut phones = Vec::new();
    for index in 0..3 {
        let (_, mut phone) = f.phone().await;
        set_token(&mut phone, 1, &format!("fcm:device-{index}")).await;
        phones.push(phone);
    }
    let mut node = f.node().await;
    // Malformed is held for no time at all, so only sharing can spare the
    // endpoint a second request.
    google.answer_token(Answer {
        status: 200,
        body: json!({"not": "a token"}),
        retry_after: None,
    });
    let held = google.hold("/token").await;

    node.reply(&mut phones[0], 2, "news").await;
    // The three sends start together; on this one-thread runtime the other
    // two wait on the cache before the stub can answer the first.
    google.wait_for(1).await;
    drop(held);
    // Each device backs off, then one asks again and the rest share its
    // token.
    f.drained().await;
    let requests = google.settled().await;
    let fetches: Vec<&Recorded> = requests.iter().filter(|r| r.path == "/token").collect();
    assert_eq!(fetches.len(), 2, "{requests:?}");
    assert!(
        fetches[1].at - fetches[0].at >= Duration::from_secs(2),
        "one failed fetch answers every send that waited on it; only a retry \
         after the backoff asks again"
    );
    assert_eq!(requests.iter().filter(|r| r.is_fcm()).count(), 3);
    f.stop().await;
    google.stop();
}

// --- Revocation ---

#[tokio::test]
async fn revoking_a_device_deletes_its_token_and_later_records_wake_nothing() {
    let google = Google::start().await;
    let f = Fixture::start(&google, Push::Fcm(Duration::ZERO)).await;
    let (phone, mut phone_socket) = f.phone().await;
    set_token(&mut phone_socket, 1, "fcm:phone-token").await;
    let (_, mut laptop) = f.phone().await;
    let mut node = f.node().await;

    f.coordinator
        .revoke_device(phone.device_id)
        .await
        .expect("revoke the phone");
    assert!(f.tokens().is_empty(), "revocation deletes the token row");
    let closed = timeout(WAIT, async {
        loop {
            match phone_socket.next().await {
                None | Some(Err(_) | Ok(Message::Close(_))) => return,
                Some(Ok(_)) => {}
            }
        }
    })
    .await;
    assert!(closed.is_ok(), "the revoked device's socket closes");

    node.reply(&mut laptop, 2, "news").await;
    f.drained().await;
    assert!(
        google.settled().await.is_empty(),
        "a revoked device is not woken"
    );
    f.stop().await;
    google.stop();
}

// --- Discovery ---

#[tokio::test]
async fn a_task_summary_carries_the_sequence_attached_reports() {
    let google = Google::start().await;
    let f = Fixture::start(&google, Push::Disabled).await;
    let (_, mut phone) = f.phone().await;
    let through = |message: ServerMessage| match message {
        ServerMessage::TaskList { tasks, .. } => {
            assert_eq!(tasks.len(), 1);
            tasks[0].through_sequence
        }
        other => panic!("expected a task list, received {other:?}"),
    };
    send(&mut phone, &ClientMessage::ListTasks { request_id: 1 }).await;
    assert_eq!(through(receive(&mut phone).await), None, "an empty task");

    let mut node = f.node().await;
    node.reply(&mut phone, 2, "news").await;
    send(&mut phone, &ClientMessage::ListTasks { request_id: 3 }).await;
    let listed = through(receive(&mut phone).await);
    // One submitted command and four execution records: sequences 0 to 4.
    assert_eq!(listed, Some(4));
    send(
        &mut phone,
        &ClientMessage::Attach {
            request_id: 4,
            task_id: f.task_id,
            after_sequence: None,
            live: false,
        },
    )
    .await;
    assert_eq!(
        receive(&mut phone).await,
        ServerMessage::Attached {
            request_id: 4,
            task_id: f.task_id,
            through_sequence: listed,
        }
    );
    f.stop().await;
    google.stop();
}

// --- Push turned off ---

#[tokio::test]
async fn without_a_service_account_tokens_register_and_nothing_is_sent() {
    let google = Google::start().await;
    let f = Fixture::start(&google, Push::Disabled).await;
    let (_, mut phone) = f.phone().await;
    assert!(matches!(
        set_token(&mut phone, 1, "fcm:phone-token").await,
        ServerMessage::PushTokenSet { .. }
    ));
    let mut node = f.node().await;
    let mut run = node.start(&mut phone, 2).await;
    node.publish(
        &mut run,
        vec![
            ExecutionEventKind::ExecutionStarted,
            ExecutionEventKind::AssistantMessage {
                text: "one".to_owned(),
            },
        ],
    )
    .await;
    for text in ["two", "three"] {
        node.publish(
            &mut run,
            vec![ExecutionEventKind::AssistantMessage {
                text: text.to_owned(),
            }],
        )
        .await;
        assert!(f.owed_wakes() <= 1, "one marker per principal at most");
    }
    f.drained().await;
    assert!(google.settled().await.is_empty(), "no outbound request");
    assert_eq!(f.tokens().len(), 1);
    f.stop().await;
    google.stop();
}

// --- Storage ---

#[tokio::test]
async fn a_schema_13_database_gains_the_push_tables_and_keeps_its_devices() {
    let google = Google::start().await;
    let mut f = Fixture::start(&google, Push::Disabled).await;
    let (phone, _) = f.phone().await;
    f.restart_after(&google, Push::Disabled, |database| {
        database
            .execute_batch(
                "DROP TABLE push_tokens; DROP TABLE push_wakes; PRAGMA user_version = 13;",
            )
            .expect("return the database to schema 13");
    })
    .await;
    let version: i64 = f
        .database()
        .query_row("PRAGMA user_version", [], |row| row.get(0))
        .expect("version");
    assert_eq!(version, 17);
    let mut socket = f.connect(&phone).await;
    assert!(matches!(
        set_token(&mut socket, 1, "fcm:phone-token").await,
        ServerMessage::PushTokenSet { .. }
    ));
    assert_eq!(f.tokens().len(), 1);
    f.stop().await;
    google.stop();
}

// --- Wakes a node asks for ---

impl Fixture {
    /// The fixture's node, enrolled with the owner recorded as its owner.
    async fn owned_node(&self) -> Node {
        let token = self
            .coordinator
            .create_node_enrollment(
                self.node_id,
                self.owner,
                SystemTime::now() + Duration::from_mins(1),
            )
            .await
            .expect("create the node's enrollment");
        let credentials = self.claim(token).await;
        Node {
            socket: self.connect(&credentials).await,
            task_id: self.task_id,
        }
    }

    fn requested_ids(&self) -> i64 {
        self.database()
            .query_row("SELECT count(*) FROM push_wake_requests", [], |row| {
                row.get(0)
            })
            .expect("count requested wakes")
    }
}

async fn request_wake(
    node: &mut Socket,
    request_id: u64,
    wake_id: Uuid,
    urgency: WakeUrgency,
) -> ServerMessage {
    send(
        node,
        &ClientMessage::RequestWake {
            request_id,
            wake_id,
            urgency,
        },
    )
    .await;
    receive(node).await
}

#[tokio::test]
async fn a_requested_wake_buzzes_or_rings_and_a_repeated_id_adds_nothing() {
    let google = Google::start().await;
    let f = Fixture::start(&google, Push::Fcm(Duration::ZERO)).await;
    let (_, mut phone) = f.phone().await;
    set_token(&mut phone, 1, "fcm:phone-token").await;
    let mut node = f.owned_node().await;
    let update = Uuid::new_v4();
    let ask = Uuid::new_v4();

    assert_eq!(
        request_wake(&mut node.socket, 1, update, WakeUrgency::Soon).await,
        ServerMessage::WakeRequested { request_id: 1 }
    );
    google.wait_for_fcm(1).await;
    f.drained().await;
    // The same id again, even louder, is acknowledged and records nothing.
    assert_eq!(
        request_wake(&mut node.socket, 2, update, WakeUrgency::Now).await,
        ServerMessage::WakeRequested { request_id: 2 }
    );
    assert_eq!(f.owed_wakes(), 0, "a repeated id owes no wake");
    assert_eq!(
        request_wake(&mut node.socket, 3, ask, WakeUrgency::Now).await,
        ServerMessage::WakeRequested { request_id: 3 }
    );
    google.wait_for_fcm(2).await;
    f.drained().await;

    let sent = google.settled().await;
    assert_eq!(
        bodies(&sent),
        [wake_body("fcm:phone-token"), ring_body("fcm:phone-token")],
        "`soon` sends the constant wake, `now` the constant ring, and the repeat nothing"
    );
    for request in &sent {
        let everything = request.everything();
        for absent in [update.to_string(), ask.to_string(), f.owner.to_string()] {
            assert!(!everything.contains(&absent), "{absent} in {everything}");
        }
    }
    assert_eq!(f.requested_ids(), 2);
    f.stop().await;
    google.stop();
}

#[tokio::test]
async fn a_wake_id_is_remembered_for_a_day_and_wakes_again_after_it() {
    let google = Google::start().await;
    let f = Fixture::start(&google, Push::Disabled).await;
    let mut node = f.owned_node().await;
    let wake = Uuid::new_v4();
    request_wake(&mut node.socket, 1, wake, WakeUrgency::Soon).await;
    let generation = || -> i64 {
        f.database()
            .query_row(
                "SELECT coalesce(max(generation), 0) FROM push_wakes",
                [],
                |row| row.get(0),
            )
            .expect("generation")
    };
    assert_eq!(generation(), 1);
    request_wake(&mut node.socket, 2, wake, WakeUrgency::Soon).await;
    assert_eq!(generation(), 1, "a repeat within the day records nothing");
    f.database()
        .execute(
            "UPDATE push_wake_requests SET received_at_ms = received_at_ms - 86400001",
            [],
        )
        .expect("age the request past a day");
    assert_eq!(
        request_wake(&mut node.socket, 3, wake, WakeUrgency::Soon).await,
        ServerMessage::WakeRequested { request_id: 3 }
    );
    assert_eq!(generation(), 2, "a repeat after a day is one more wake");
    assert_eq!(
        f.requested_ids(),
        1,
        "the forgotten id was pruned and recorded anew"
    );
    f.stop().await;
    google.stop();
}

#[tokio::test]
async fn a_node_without_an_owner_is_refused_and_records_no_wake() {
    let google = Google::start().await;
    let f = Fixture::start(&google, Push::Fcm(Duration::ZERO)).await;
    let (_, mut phone) = f.phone().await;
    set_token(&mut phone, 1, "fcm:phone-token").await;
    let mut node = f.node().await;
    let answer = request_wake(&mut node.socket, 7, Uuid::new_v4(), WakeUrgency::Now).await;
    assert_eq!(
        refused(answer, 7, ErrorCode::InvalidRole),
        format!("node {} has no owner, so it has no one to wake", f.node_id)
    );
    assert_eq!(f.owed_wakes(), 0);
    assert_eq!(f.requested_ids(), 0);
    assert!(google.settled().await.is_empty(), "nothing is sent");
    f.stop().await;
    google.stop();
}

#[tokio::test]
async fn a_ring_stays_owed_through_the_wakes_around_it_and_rings_once() {
    let google = Google::start().await;
    let f = Fixture::start(&google, Push::Fcm(Duration::ZERO)).await;
    let (_, mut phone) = f.phone().await;
    set_token(&mut phone, 1, "fcm:phone-token").await;
    let mut node = f.owned_node().await;
    let held = google.hold("fcm:phone-token").await;
    node.reply(&mut phone, 2, "first").await;
    google.wait_for_fcm(1).await;
    // A ring and then a reply arrive while the first wake is in flight; the
    // reply's wake must not replace the ring.
    request_wake(&mut node.socket, 3, Uuid::new_v4(), WakeUrgency::Now).await;
    node.reply(&mut phone, 4, "second").await;
    drop(held);
    google.wait_for_fcm(2).await;
    f.drained().await;
    // Once the phone rang, a later wake only buzzes it.
    node.reply(&mut phone, 5, "third").await;
    google.wait_for_fcm(3).await;
    f.drained().await;
    assert_eq!(
        bodies(&google.settled().await),
        [
            wake_body("fcm:phone-token"),
            ring_body("fcm:phone-token"),
            wake_body("fcm:phone-token"),
        ]
    );
    f.stop().await;
    google.stop();
}

#[tokio::test]
async fn a_wake_recorded_while_a_ring_is_in_flight_only_buzzes_the_phone_it_rang() {
    let google = Google::start().await;
    let f = Fixture::start(&google, Push::Fcm(Duration::ZERO)).await;
    let (_, mut phone) = f.phone().await;
    set_token(&mut phone, 1, "fcm:phone-token").await;
    let mut node = f.owned_node().await;
    let held = google.hold("fcm:phone-token").await;
    request_wake(&mut node.socket, 2, Uuid::new_v4(), WakeUrgency::Now).await;
    google.wait_for_fcm(1).await;
    // Recorded during the ring's send, so the phone is owed one more wake,
    // and the ring it is getting already counts.
    node.reply(&mut phone, 3, "news").await;
    drop(held);
    google.wait_for_fcm(2).await;
    f.drained().await;
    assert_eq!(
        bodies(&google.settled().await),
        [ring_body("fcm:phone-token"), wake_body("fcm:phone-token")]
    );
    f.stop().await;
    google.stop();
}

#[tokio::test]
async fn a_ring_owed_at_a_restart_rings_once_after_it() {
    let push = Push::Fcm(Duration::from_mins(1));
    let google = Google::start().await;
    let mut f = Fixture::start(&google, push).await;
    let (_, mut phone) = f.phone().await;
    set_token(&mut phone, 1, "fcm:phone-token").await;
    let mut node = f.owned_node().await;
    node.reply(&mut phone, 2, "first").await;
    google.wait_for_fcm(1).await;
    // Inside the gap, so the ring is owed and not sent before the restart.
    request_wake(&mut node.socket, 3, Uuid::new_v4(), WakeUrgency::Now).await;
    assert_eq!(f.owed_wakes(), 1);
    drop((phone, node));

    f.restart(&google, push).await;
    google.wait_for_fcm(2).await;
    f.drained().await;
    assert_eq!(
        bodies(&google.settled().await),
        [wake_body("fcm:phone-token"), ring_body("fcm:phone-token")],
        "exactly one ring after the restart"
    );
    f.stop().await;
    google.stop();
}

#[tokio::test]
async fn a_schema_14_database_learns_rings_and_rings_none_of_its_wakes() {
    let google = Google::start().await;
    let mut f = Fixture::start(&google, Push::Disabled).await;
    let stored = PrincipalId::new();
    f.restart_after(&google, Push::Disabled, |database| {
        database
            .execute_batch(&format!(
                "ALTER TABLE push_wakes DROP COLUMN ring_generation;
                 DROP TABLE push_wake_requests;
                 INSERT INTO push_wakes (principal_id, generation, settled) VALUES ('{stored}', 3, 3);
                 PRAGMA user_version = 14;"
            ))
            .expect("return the database to schema 14");
    })
    .await;
    let database = f.database();
    let version: i64 = database
        .query_row("PRAGMA user_version", [], |row| row.get(0))
        .expect("version");
    assert_eq!(version, 17);
    let ring: i64 = database
        .query_row(
            "SELECT ring_generation FROM push_wakes WHERE principal_id = ?1",
            [stored.to_string()],
            |row| row.get(0),
        )
        .expect("the stored wake row");
    assert_eq!(ring, 0, "a stored wake rings at no generation");
    assert_eq!(f.requested_ids(), 0);
    drop(database);
    f.stop().await;
    google.stop();
}

// --- Secrets and telemetry ---

/// Lines of the test key's base64 body; any one of them in an output would
/// show the key leaked.
fn key_lines() -> Vec<&'static str> {
    TEST_KEY
        .lines()
        .filter(|line| !line.starts_with("-----"))
        .collect()
}

/// Runs every push path that logs, against the stand-in, and checks the
/// SQLite files hold no key or access token. Its log is read by
/// `every_push_log_line_names_no_secret`, which runs it in a child process.
#[tokio::test]
#[ignore = "run by every_push_log_line_names_no_secret, which reads its log"]
async fn push_log_scenario() {
    let google = Google::start().await;
    // A rejected sign-in is remembered for a minute, so it has a
    // coordinator of its own.
    let rejected = Fixture::start(&google, Push::Fcm(Duration::ZERO)).await;
    let (_, mut phone) = rejected.phone().await;
    let mut node = rejected.node().await;
    set_token(&mut phone, 1, "fcm:phone-token").await;
    google.answer_token(Answer {
        status: 401,
        body: json!({"error": "invalid_client"}),
        retry_after: None,
    });
    node.reply(&mut phone, 2, "rejected sign-in").await;
    google.wait_for(1).await;
    // The wake stays owed; the quiet moment lets its failure be logged.
    google.settled().await;
    drop((phone, node));
    let rejected_files = rejected.stop().await;

    let f = Fixture::start(&google, Push::Fcm(Duration::ZERO)).await;
    let (_, mut phone) = f.phone().await;
    let (_, mut stranger) = f.phone_of(PrincipalId::new()).await;
    let mut node = f.node().await;
    set_token(&mut phone, 1, "fcm:phone-token").await;
    set_token(&mut stranger, 2, "fcm:phone-token").await;

    google.answer_fcm(Answer::fcm_error(
        403,
        "SENDER_ID_MISMATCH",
        "PERMISSION_DENIED",
    ));
    node.reply(&mut phone, 4, "sender mismatch").await;
    google.wait_for_fcm(1).await;
    f.drained().await;

    node.reply(&mut phone, 5, "delivered").await;
    google.wait_for_fcm(2).await;
    f.drained().await;

    google.answer_fcm(Answer::fcm_error(404, "UNREGISTERED", "NOT_FOUND"));
    node.reply(&mut phone, 6, "dead token").await;
    google.wait_for_fcm(3).await;
    f.drained().await;
    set_token(&mut phone, 7, "fcm:phone-token").await;
    send(&mut phone, &ClientMessage::ClearPushToken { request_id: 8 }).await;
    receive(&mut phone).await;
    drop((phone, stranger, node));
    let files = f.stop().await;

    let disabled = Fixture::start(&google, Push::Disabled).await;
    let (_, mut phone) = disabled.phone().await;
    let mut node = disabled.node().await;
    node.reply(&mut phone, 1, "push is off").await;
    disabled.drained().await;
    drop((phone, node));
    let disabled_files = disabled.stop().await;

    for directory in [rejected_files, files, disabled_files] {
        for entry in std::fs::read_dir(directory.path()).expect("read the coordinator's files") {
            let path = entry.expect("entry").path();
            if !path.to_string_lossy().contains("control.sqlite") {
                continue;
            }
            let bytes = String::from_utf8_lossy(&std::fs::read(&path).expect("read")).into_owned();
            for line in key_lines() {
                assert!(
                    !bytes.contains(line),
                    "{} holds key material",
                    path.display()
                );
            }
            assert!(
                !bytes.contains(ACCESS_TOKEN),
                "{} holds an access token",
                path.display()
            );
        }
    }
    google.stop();
}

#[test]
fn every_push_log_line_names_no_secret() {
    let output = std::process::Command::new(std::env::current_exe().expect("test binary"))
        .args([
            "push_log_scenario",
            "--exact",
            "--ignored",
            "--nocapture",
            "--test-threads",
            "1",
        ])
        .output()
        .expect("run the log scenario");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        output.status.success(),
        "the log scenario passes:\n{}\n{stderr}",
        String::from_utf8_lossy(&output.stdout)
    );
    let records: Vec<Value> = stderr
        .lines()
        .filter_map(|line| serde_json::from_str::<Value>(line).ok())
        .filter(|record| record["component"] == "renoa.coordinator")
        .collect();
    let names: Vec<&str> = records
        .iter()
        .filter_map(|record| record["event"].as_str())
        .collect();
    for expected in [
        "push_token_set",
        "push_token_conflict",
        "push_token_cleared",
        "push_wake_recorded",
        "push_wake_sent",
        "push_wake_failed",
        "push_token_dropped",
        "push_disabled",
    ] {
        assert!(names.contains(&expected), "{expected} is logged: {names:?}");
    }
    let reasons: Vec<&str> = records
        .iter()
        .filter(|record| record["event"] == "push_wake_failed")
        .filter_map(|record| record["fields"]["reason"].as_str())
        .collect();
    assert!(reasons.contains(&"SENDER_ID_MISMATCH"), "{reasons:?}");
    assert!(
        reasons.contains(&"the token endpoint rejected the service account"),
        "{reasons:?}"
    );
    for record in records
        .iter()
        .filter(|record| record["event"] == "push_token_dropped")
    {
        assert_eq!(
            record["level"], "warn",
            "a dropped token is a warning: {record}"
        );
    }
    for record in records.iter().filter(|record| {
        record["event"]
            .as_str()
            .is_some_and(|name| name.starts_with("push_"))
    }) {
        let fields = &record["fields"];
        assert!(
            fields.get("device_id").is_some() || fields.get("principal_id").is_some(),
            "{record}"
        );
        assert!(fields.get("reason").is_some(), "{record}");
    }
    for line in stderr.lines() {
        for secret in key_lines()
            .into_iter()
            .chain([ACCESS_TOKEN, "fcm:phone-token"])
        {
            assert!(!line.contains(secret), "a log line holds a secret: {line}");
        }
    }
}
