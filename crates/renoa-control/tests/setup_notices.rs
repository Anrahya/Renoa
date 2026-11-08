//! Setup notices sealed to a signed-in app's device key, through a real
//! coordinator over WebSocket and its SQLite file.

use std::time::{Duration, SystemTime, UNIX_EPOCH};

use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use futures_util::{SinkExt as _, StreamExt as _};
use renoa_control::{
    Coordinator, MAX_LISTED_NOTICES, MAX_SEALED_NOTICE_BYTES, PeerIdentity, TaskSpec,
};
use renoa_protocol::{PrincipalId, SurfaceRef, TargetRef};
use renoa_rcp_wire::{
    ClientMessage, DeviceCredentials, DeviceNoticeKey, ErrorCode, HeldNotice, JSON_WS_VERSION,
    NodeId, NoticeKey, SealedNotice, ServerMessage, TaskId,
};
use ring::{
    rand::SystemRandom,
    signature::{ECDSA_P256_SHA256_ASN1_SIGNING, EcdsaKeyPair, KeyPair as _},
};
use rusqlite::Connection;
use tempfile::TempDir;
use tokio::{net::TcpListener, task::JoinHandle, time::timeout};
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream, connect_async, tungstenite::Message};
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

type Socket = WebSocketStream<MaybeTlsStream<tokio::net::TcpStream>>;

fn surface(principal_id: PrincipalId) -> PeerIdentity {
    PeerIdentity::Surface {
        principal_id,
        surface: SurfaceRef::new("android"),
    }
}

const WAIT: Duration = Duration::from_secs(10);

struct Fixture {
    files: TempDir,
    coordinator: Coordinator,
    ws: String,
    stop: CancellationToken,
    task: JoinHandle<Result<(), renoa_control::ControlError>>,
    owner: PrincipalId,
    node_id: NodeId,
}

impl Fixture {
    async fn start() -> Self {
        let files = TempDir::new().expect("temporary directory");
        let owner = PrincipalId::new();
        let node_id = NodeId::new();
        let coordinator = Coordinator::open_with_passkeys(
            files.path().join("control.sqlite"),
            "localhost",
            "http://localhost",
        )
        .expect("open the coordinator");
        coordinator
            .create_task(TaskSpec {
                task_id: TaskId::new(),
                principal_id: owner,
                node_id,
                target: TargetRef::new("agent:scout"),
            })
            .await
            .expect("the node's owner");
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("listener");
        let ws = format!("ws://{}/connect", listener.local_addr().expect("address"));
        let stop = CancellationToken::new();
        let task = tokio::spawn(coordinator.clone().serve(listener, stop.clone()));
        Self {
            files,
            coordinator,
            ws,
            stop,
            task,
            owner,
            node_id,
        }
    }

    async fn stop(self) {
        self.stop.cancel();
        self.task
            .await
            .expect("coordinator task")
            .expect("coordinator stops");
    }

    fn database(&self) -> Connection {
        Connection::open(self.files.path().join("control.sqlite")).expect("open the database")
    }

    async fn enroll(&self, peer: PeerIdentity) -> DeviceCredentials {
        let token = self
            .coordinator
            .create_enrollment(peer, SystemTime::now() + Duration::from_mins(1))
            .await
            .expect("create an enrollment");
        self.claim(token).await
    }

    async fn claim(&self, token: renoa_rcp_wire::EnrollmentToken) -> DeviceCredentials {
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
        let mut socket = self.authenticate(credentials).await;
        assert_eq!(
            receive(&mut socket).await,
            ServerMessage::Authenticated {
                version: JSON_WS_VERSION
            }
        );
        socket
    }

    async fn authenticate(&self, credentials: &DeviceCredentials) -> Socket {
        let (mut socket, _) = connect_async(&self.ws).await.expect("connect");
        send(
            &mut socket,
            &ClientMessage::Authenticate {
                version: JSON_WS_VERSION,
                credentials: credentials.clone(),
            },
        )
        .await;
        socket
    }

    /// A phone of `principal_id` that proves its device key, as the app does.
    async fn phone_of(&self, principal_id: PrincipalId) -> (DeviceCredentials, Socket) {
        let credentials = self.enroll(surface(principal_id)).await;
        let rng = SystemRandom::new();
        let pkcs8 = EcdsaKeyPair::generate_pkcs8(&ECDSA_P256_SHA256_ASN1_SIGNING, &rng)
            .expect("generate the device key");
        let key = EcdsaKeyPair::from_pkcs8(&ECDSA_P256_SHA256_ASN1_SIGNING, pkcs8.as_ref(), &rng)
            .expect("load the device key");
        self.database()
            .execute(
                "INSERT INTO device_keys (device_id, principal_id, public_key, label, created_at_ms)
                 VALUES (?1, ?2, ?3, 'phone', 0)",
                rusqlite::params![
                    credentials.device_id.to_string(),
                    principal_id.to_string(),
                    key.public_key().as_ref(),
                ],
            )
            .expect("bind the device key");
        let mut socket = self.authenticate(&credentials).await;
        let ServerMessage::DeviceChallenge { nonce } = receive(&mut socket).await else {
            panic!("a key-bound device is challenged");
        };
        let proof = format!("renoa device proof v1\n{}\n{nonce}", credentials.device_id);
        let signature = key.sign(&rng, proof.as_bytes()).expect("sign the proof");
        send(
            &mut socket,
            &ClientMessage::DeviceProof {
                signature: URL_SAFE_NO_PAD.encode(signature.as_ref()),
            },
        )
        .await;
        assert_eq!(
            receive(&mut socket).await,
            ServerMessage::Authenticated {
                version: JSON_WS_VERSION
            }
        );
        (credentials, socket)
    }

    /// A device of `principal_id` that presents only its bearer credential.
    async fn bearer_device_of(&self, principal_id: PrincipalId) -> Socket {
        let credentials = self.enroll(surface(principal_id)).await;
        self.connect(&credentials).await
    }

    /// The owner's node, enrolled for them.
    async fn node(&self) -> Socket {
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
        self.connect(&credentials).await
    }
}

async fn send(socket: &mut Socket, message: &ClientMessage) {
    let json = serde_json::to_string(message).expect("serialize client message");
    socket
        .send(Message::Text(json.into()))
        .await
        .expect("send client message");
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

async fn ask(socket: &mut Socket, message: ClientMessage) -> ServerMessage {
    send(socket, &message).await;
    receive(socket).await
}

fn refused(message: &ServerMessage, code: ErrorCode) -> String {
    match message {
        ServerMessage::Error {
            code: refused,
            message,
            ..
        } if *refused == code => message.clone(),
        other => panic!("expected a {code:?} refusal, got {other:?}"),
    }
}

fn key(byte: u8) -> NoticeKey {
    let mut point = [byte; 65];
    point[0] = 0x04;
    NoticeKey::new(URL_SAFE_NO_PAD.encode(point))
}

fn later_ms() -> i64 {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock")
        .as_millis();
    i64::try_from(now).expect("millis") + 600_000
}

fn notice(device: &DeviceCredentials, sealed: &str) -> SealedNotice {
    SealedNotice {
        notice_id: Uuid::new_v4(),
        device_id: device.device_id,
        sealed: sealed.to_owned(),
        expires_at_ms: later_ms(),
    }
}

async fn set_key(socket: &mut Socket, public_key: NoticeKey) -> ServerMessage {
    ask(
        socket,
        ClientMessage::SetNoticeKey {
            request_id: 1,
            public_key,
        },
    )
    .await
}

async fn listed(socket: &mut Socket) -> Vec<HeldNotice> {
    match ask(socket, ClientMessage::ListNotices { request_id: 2 }).await {
        ServerMessage::NoticeList { notices, .. } => notices,
        other => panic!("a notice list, got {other:?}"),
    }
}

async fn keys(node: &mut Socket) -> Vec<DeviceNoticeKey> {
    match ask(node, ClientMessage::ListNoticeKeys { request_id: 3 }).await {
        ServerMessage::NoticeKeys { keys, .. } => keys,
        other => panic!("notice keys, got {other:?}"),
    }
}

async fn put(node: &mut Socket, notice: SealedNotice) -> ServerMessage {
    ask(
        node,
        ClientMessage::PutNotice {
            request_id: 4,
            notice,
        },
    )
    .await
}

#[tokio::test]
async fn a_node_seals_only_to_its_owners_active_devices_with_a_key() {
    let fixture = Fixture::start().await;
    let (phone, mut phone_socket) = fixture.phone_of(fixture.owner).await;
    let (_, mut keyless) = fixture.phone_of(fixture.owner).await;
    let (_, mut stranger) = fixture.phone_of(PrincipalId::new()).await;
    let bad = set_key(&mut phone_socket, NoticeKey::new("not-a-key")).await;
    assert!(refused(&bad, ErrorCode::InvalidMessage).contains("uncompressed P-256 point"));
    assert_eq!(
        set_key(&mut phone_socket, key(1)).await,
        ServerMessage::NoticeKeySet { request_id: 1 }
    );
    assert_eq!(
        set_key(&mut phone_socket, key(2)).await,
        ServerMessage::NoticeKeySet { request_id: 1 },
        "a new key replaces the old"
    );
    assert_eq!(
        set_key(&mut stranger, key(3)).await,
        ServerMessage::NoticeKeySet { request_id: 1 }
    );
    assert!(listed(&mut keyless).await.is_empty());
    let mut node = fixture.node().await;
    assert_eq!(
        keys(&mut node).await,
        vec![DeviceNoticeKey {
            device_id: phone.device_id,
            public_key: key(2),
        }]
    );
    fixture.stop().await;
}

#[tokio::test]
async fn a_notice_reaches_its_device_once_and_goes_when_acknowledged_or_withdrawn() {
    let fixture = Fixture::start().await;
    let (phone, mut phone_socket) = fixture.phone_of(fixture.owner).await;
    let (tablet, mut tablet_socket) = fixture.phone_of(fixture.owner).await;
    set_key(&mut phone_socket, key(1)).await;
    set_key(&mut tablet_socket, key(2)).await;
    let mut node = fixture.node().await;

    let first = notice(&phone, "c2VhbGVk");
    assert_eq!(
        put(&mut node, first.clone()).await,
        ServerMessage::NoticePut { request_id: 4 }
    );
    let retried = SealedNotice {
        sealed: "c2VhbGVkIGFnYWlu".to_owned(),
        ..first.clone()
    };
    assert_eq!(
        put(&mut node, retried).await,
        ServerMessage::NoticePut { request_id: 4 },
        "a retried put is answered and stores nothing new"
    );
    let elsewhere = SealedNotice {
        device_id: tablet.device_id,
        ..first.clone()
    };
    assert!(
        refused(&put(&mut node, elsewhere).await, ErrorCode::Conflict)
            .contains("another recipient")
    );
    assert_eq!(
        listed(&mut phone_socket).await,
        vec![HeldNotice {
            notice_id: first.notice_id,
            sealed: first.sealed.clone(),
            expires_at_ms: first.expires_at_ms,
        }]
    );
    assert!(listed(&mut tablet_socket).await.is_empty(), "only its own");

    assert_eq!(
        ask(
            &mut tablet_socket,
            ClientMessage::AcknowledgeNotice {
                request_id: 5,
                notice_id: first.notice_id,
            },
        )
        .await,
        ServerMessage::NoticeAcknowledged { request_id: 5 }
    );
    assert_eq!(
        listed(&mut phone_socket).await.len(),
        1,
        "another device acknowledges nothing"
    );
    ask(
        &mut phone_socket,
        ClientMessage::AcknowledgeNotice {
            request_id: 5,
            notice_id: first.notice_id,
        },
    )
    .await;
    assert!(listed(&mut phone_socket).await.is_empty());

    let second = notice(&tablet, "c2Vjb25k");
    put(&mut node, second.clone()).await;
    assert_eq!(
        ask(
            &mut node,
            ClientMessage::WithdrawNotices {
                request_id: 6,
                notice_ids: vec![second.notice_id],
            },
        )
        .await,
        ServerMessage::NoticesWithdrawn { request_id: 6 }
    );
    assert!(listed(&mut tablet_socket).await.is_empty());
    fixture.stop().await;
}

#[tokio::test]
async fn a_wrong_notice_is_refused_before_anything_is_stored() {
    let fixture = Fixture::start().await;
    let (phone, mut phone_socket) = fixture.phone_of(fixture.owner).await;
    let (keyless, _) = fixture.phone_of(fixture.owner).await;
    let (stranger, mut stranger_socket) = fixture.phone_of(PrincipalId::new()).await;
    set_key(&mut phone_socket, key(1)).await;
    set_key(&mut stranger_socket, key(2)).await;
    let mut node = fixture.node().await;
    let expired = SealedNotice {
        expires_at_ms: 1,
        ..notice(&phone, "c2VhbGVk")
    };
    assert!(refused(&put(&mut node, expired).await, ErrorCode::InvalidMessage).contains("expired"));
    let oversized = notice(&phone, &"A".repeat(MAX_SEALED_NOTICE_BYTES + 1));
    assert!(
        refused(&put(&mut node, oversized).await, ErrorCode::InvalidMessage).contains("base64url")
    );
    for device in [&keyless, &stranger] {
        let message = put(&mut node, notice(device, "c2VhbGVk")).await;
        assert!(refused(&message, ErrorCode::InvalidRole).contains("list the notice keys again"));
    }
    let stored: i64 = fixture
        .database()
        .query_row("SELECT count(*) FROM setup_notices", [], |row| row.get(0))
        .expect("count notices");
    assert_eq!(stored, 0);
    fixture.stop().await;
}

#[tokio::test]
async fn a_list_carries_at_most_the_bound_and_the_rest_after_acknowledging() {
    let fixture = Fixture::start().await;
    let (phone, mut phone_socket) = fixture.phone_of(fixture.owner).await;
    set_key(&mut phone_socket, key(1)).await;
    let mut node = fixture.node().await;
    for _ in 0..=MAX_LISTED_NOTICES {
        put(&mut node, notice(&phone, "c2VhbGVk")).await;
    }
    let first = listed(&mut phone_socket).await;
    assert_eq!(first.len(), MAX_LISTED_NOTICES);
    ask(
        &mut phone_socket,
        ClientMessage::AcknowledgeNotice {
            request_id: 5,
            notice_id: first[0].notice_id,
        },
    )
    .await;
    let rest = listed(&mut phone_socket).await;
    assert_eq!(rest.len(), MAX_LISTED_NOTICES);
    assert!(!rest.iter().any(|held| held.notice_id == first[0].notice_id));
    fixture.stop().await;
}

#[tokio::test]
async fn a_device_that_did_not_prove_its_key_holds_no_notices() {
    let fixture = Fixture::start().await;
    let mut bearer = fixture.bearer_device_of(fixture.owner).await;
    let refused_key = set_key(&mut bearer, key(1)).await;
    assert!(
        refused(&refused_key, ErrorCode::InvalidRole).contains("proved its device key"),
        "{refused_key:?}"
    );
    let listing = ask(&mut bearer, ClientMessage::ListNotices { request_id: 2 }).await;
    refused(&listing, ErrorCode::InvalidRole);
    let mut node = fixture.node().await;
    assert!(keys(&mut node).await.is_empty());
    fixture.stop().await;
}

#[tokio::test]
async fn a_node_withdraws_only_the_notices_it_put() {
    let fixture = Fixture::start().await;
    let (phone, mut phone_socket) = fixture.phone_of(fixture.owner).await;
    set_key(&mut phone_socket, key(1)).await;
    let mut node = fixture.node().await;
    let held = notice(&phone, "c2VhbGVk");
    put(&mut node, held.clone()).await;
    let token = fixture
        .coordinator
        .create_node_enrollment(
            NodeId::new(),
            fixture.owner,
            SystemTime::now() + Duration::from_mins(1),
        )
        .await
        .expect("create the other node's enrollment");
    let credentials = fixture.claim(token).await;
    let mut other = fixture.connect(&credentials).await;
    assert_eq!(
        ask(
            &mut other,
            ClientMessage::WithdrawNotices {
                request_id: 6,
                notice_ids: vec![held.notice_id],
            },
        )
        .await,
        ServerMessage::NoticesWithdrawn { request_id: 6 }
    );
    let listed = listed(&mut phone_socket).await;
    assert_eq!(
        listed
            .iter()
            .map(|notice| notice.notice_id)
            .collect::<Vec<_>>(),
        [held.notice_id]
    );
    fixture.stop().await;
}

#[tokio::test]
async fn revoking_a_device_forgets_its_key_and_notices() {
    let fixture = Fixture::start().await;
    let (phone, mut phone_socket) = fixture.phone_of(fixture.owner).await;
    set_key(&mut phone_socket, key(1)).await;
    let mut node = fixture.node().await;
    put(&mut node, notice(&phone, "c2VhbGVk")).await;
    fixture
        .coordinator
        .revoke_device(phone.device_id)
        .await
        .expect("revoke the phone");
    assert!(keys(&mut node).await.is_empty());
    let database = fixture.database();
    for table in ["notice_keys", "setup_notices"] {
        let rows: i64 = database
            .query_row(&format!("SELECT count(*) FROM {table}"), [], |row| {
                row.get(0)
            })
            .expect("count rows");
        assert_eq!(rows, 0, "{table}");
    }
    fixture.stop().await;
}

#[tokio::test]
async fn a_schema_15_database_gains_the_notice_tables() {
    let files = TempDir::new().expect("temporary directory");
    let path = files.path().join("control.sqlite");
    drop(Coordinator::open_with_passkeys(&path, "localhost", "http://localhost").expect("open"));
    let database = Connection::open(&path).expect("database");
    database
        .execute_batch(
            "DROP TABLE setup_notices; DROP TABLE notice_keys; PRAGMA user_version = 15;",
        )
        .expect("restore schema 15");
    drop(database);
    drop(Coordinator::open_with_passkeys(&path, "localhost", "http://localhost").expect("reopen"));
    let database = Connection::open(&path).expect("database");
    let version: i64 = database
        .query_row("PRAGMA user_version", [], |row| row.get(0))
        .expect("version");
    assert_eq!(version, 18);
    for table in ["notice_keys", "setup_notices"] {
        let present: bool = database
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name=?1)",
                [table],
                |row| row.get(0),
            )
            .expect("table lookup");
        assert!(present, "{table}");
    }
}
