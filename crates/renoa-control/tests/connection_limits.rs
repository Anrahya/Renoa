use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use renoa_control::Coordinator;
use tempfile::TempDir;
use tokio::net::TcpListener;
use tokio_tungstenite::{
    connect_async,
    tungstenite::{Error as WebSocketError, Message, http::StatusCode},
};
use tokio_util::sync::CancellationToken;

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
