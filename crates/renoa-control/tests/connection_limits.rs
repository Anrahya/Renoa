use std::time::{Duration, SystemTime};

use futures_util::{SinkExt, StreamExt};
use renoa_control::{
    ClientMessage, Coordinator, DeviceCredentials, EnrollmentToken, JSON_WS_VERSION, PeerIdentity,
    ServerMessage,
};
use renoa_protocol::{PrincipalId, SurfaceRef};
use tempfile::TempDir;
use tokio::net::TcpListener;
use tokio_tungstenite::{
    connect_async,
    tungstenite::{Error as WebSocketError, Message, http::StatusCode},
};
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

const AUTHENTICATION_DEADLINE: Duration = Duration::from_secs(10);
const MAX_APPLICATION_MESSAGE_BYTES: usize = 1024 * 1024;
const CONNECTION_CAPACITY: usize = 128;

#[tokio::test]
async fn connections_over_the_coordinator_budget_are_rejected_before_upgrade() {
    let server = TestServer::start().await;
    let mut connections = Vec::with_capacity(CONNECTION_CAPACITY);
    for _ in 0..CONNECTION_CAPACITY {
        let (socket, _) = connect_async(&server.url)
            .await
            .expect("connect within coordinator budget");
        connections.push(socket);
    }

    let overflow = match connect_async(&server.url).await {
        Err(WebSocketError::Http(response)) => Ok(response.status()),
        Err(error) => Err(error.to_string()),
        Ok((socket, _)) => {
            drop(socket);
            Err("overflow connection was accepted".to_owned())
        }
    };

    drop(connections);
    server.stop().await;
    assert_eq!(overflow, Ok(StatusCode::SERVICE_UNAVAILABLE));
}

#[tokio::test(start_paused = true)]
async fn an_unauthenticated_connection_is_closed_after_the_deadline() {
    let server = TestServer::start().await;
    let (mut socket, _) = connect_async(&server.url)
        .await
        .expect("connect unauthenticated peer");

    tokio::time::sleep(AUTHENTICATION_DEADLINE + Duration::from_secs(1)).await;

    expect_closed(&mut socket).await;
    server.stop().await;
}

#[tokio::test]
async fn an_oversized_application_message_is_rejected() {
    let server = TestServer::start().await;
    let (mut socket, _) = connect_async(&server.url)
        .await
        .expect("connect oversized-message peer");
    let oversized = "x".repeat(MAX_APPLICATION_MESSAGE_BYTES + 1);

    socket
        .send(Message::Text(oversized.into()))
        .await
        .expect("send oversized message");

    expect_closed(&mut socket).await;
    server.stop().await;
}

#[tokio::test]
async fn authenticated_connections_receive_transport_keepalives_without_protocol_errors() {
    let (server, token) = TestServer::start_with_surface().await;
    let credentials = enroll(&server.url, token).await;
    let (mut socket, _) = connect_async(&server.url)
        .await
        .expect("connect authenticated surface");
    send(
        &mut socket,
        &ClientMessage::Authenticate {
            version: JSON_WS_VERSION,
            credentials,
        },
    )
    .await;
    assert!(matches!(
        receive_text(&mut socket).await,
        ServerMessage::Authenticated { .. }
    ));

    let frame = tokio::time::timeout(Duration::from_secs(35), socket.next())
        .await
        .expect("coordinator should send a transport keepalive")
        .expect("coordinator closed before its keepalive")
        .expect("receive transport keepalive");
    let Message::Ping(payload) = frame else {
        panic!("expected a WebSocket ping, received {frame:?}");
    };
    socket
        .send(Message::Pong(payload))
        .await
        .expect("answer transport keepalive");
    send(&mut socket, &ClientMessage::ListTasks { request_id: 7 }).await;
    assert!(matches!(
        receive_text(&mut socket).await,
        ServerMessage::TaskList { request_id: 7, .. }
    ));
    server.stop().await;
}

async fn expect_closed<S>(socket: &mut tokio_tungstenite::WebSocketStream<S>)
where
    S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin,
{
    let closed = tokio::time::timeout(Duration::from_secs(1), socket.next())
        .await
        .expect("coordinator should close the connection");
    assert!(matches!(
        closed,
        None | Some(Ok(Message::Close(_)) | Err(_))
    ));
}

struct TestServer {
    url: String,
    shutdown: CancellationToken,
    task: tokio::task::JoinHandle<()>,
    _files: TempDir,
}

impl TestServer {
    async fn start() -> Self {
        let files = tempfile::tempdir().expect("temporary directory");
        let coordinator =
            Coordinator::open(files.path().join("control.sqlite")).expect("open coordinator");
        Self::serve(files, coordinator).await
    }

    async fn start_with_surface() -> (Self, EnrollmentToken) {
        let files = tempfile::tempdir().expect("temporary directory");
        let coordinator =
            Coordinator::open(files.path().join("control.sqlite")).expect("open coordinator");
        let token = coordinator
            .create_enrollment(
                PeerIdentity::Surface {
                    principal_id: PrincipalId::from_uuid(Uuid::from_u128(1)),
                    surface: SurfaceRef::new("keepalive-test"),
                },
                SystemTime::now() + Duration::from_mins(1),
            )
            .await
            .expect("create surface enrollment");
        (Self::serve(files, coordinator).await, token)
    }

    async fn serve(files: TempDir, coordinator: Coordinator) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0")
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
            url: format!("ws://{address}/connect"),
            shutdown,
            task,
            _files: files,
        }
    }

    async fn stop(self) {
        self.shutdown.cancel();
        self.task.await.expect("server task");
    }
}

async fn enroll(url: &str, token: EnrollmentToken) -> DeviceCredentials {
    let (mut socket, _) = connect_async(url).await.expect("connect for enrollment");
    send(
        &mut socket,
        &ClientMessage::Enroll {
            version: JSON_WS_VERSION,
            token,
        },
    )
    .await;
    let ServerMessage::Enrolled { credentials, .. } = receive_text(&mut socket).await else {
        panic!("coordinator did not enroll surface");
    };
    credentials
}

async fn send<S>(socket: &mut tokio_tungstenite::WebSocketStream<S>, message: &ClientMessage)
where
    S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin,
{
    socket
        .send(Message::Text(
            serde_json::to_string(message)
                .expect("serialize client message")
                .into(),
        ))
        .await
        .expect("send client message");
}

async fn receive_text<S>(socket: &mut tokio_tungstenite::WebSocketStream<S>) -> ServerMessage
where
    S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin,
{
    let message = socket
        .next()
        .await
        .expect("coordinator closed socket")
        .expect("receive server message");
    let Message::Text(json) = message else {
        panic!("expected a text frame, received {message:?}");
    };
    serde_json::from_str(&json).expect("parse server message")
}
