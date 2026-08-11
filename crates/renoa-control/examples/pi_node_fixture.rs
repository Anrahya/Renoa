use std::{
    io::{self, Write},
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, SystemTime},
};

use futures_util::{SinkExt, StreamExt};
use renoa_control::{
    ClientMessage, Coordinator, DeviceCredentials, JSON_WS_VERSION, NodeId, PeerIdentity,
    ServerMessage, TaskId, TaskSpec,
};
use renoa_protocol::{PrincipalId, SurfaceRef, TargetRef};
use serde_json::json;
use tokio::net::TcpListener;
use tokio_tungstenite::{
    MaybeTlsStream, WebSocketStream, accept_async, connect_async, tungstenite::Message,
};
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

type Socket = WebSocketStream<MaybeTlsStream<tokio::net::TcpStream>>;

#[tokio::main]
async fn main() {
    let database = std::env::args_os()
        .nth(1)
        .map(PathBuf::from)
        .expect("usage: pi_node_fixture <database-path>");
    let coordinator = Coordinator::open(database).expect("open coordinator");
    let principal_id = PrincipalId::from_uuid(Uuid::from_u128(1));
    let node_id = NodeId::from_uuid(Uuid::from_u128(2));
    let task_id = TaskId::from_uuid(Uuid::from_u128(3));
    coordinator
        .create_task(TaskSpec {
            task_id,
            principal_id,
            node_id,
            target: TargetRef::new("workspace:pi-test"),
        })
        .await
        .expect("create Pi fixture task");
    let (endpoint, shutdown, server) = start_coordinator(coordinator.clone()).await;
    let lossy_endpoint = start_lossy_proxy(endpoint.clone()).await;
    let node_credentials = enroll(&coordinator, &endpoint, PeerIdentity::Node { node_id }).await;
    let surface_credentials = enroll(
        &coordinator,
        &endpoint,
        PeerIdentity::Surface {
            principal_id,
            surface: SurfaceRef::new("pi_test"),
        },
    )
    .await;
    let description = json!({
        "endpoint": endpoint,
        "lossyEndpoint": lossy_endpoint,
        "nodeCredentials": node_credentials,
        "surfaceCredentials": surface_credentials,
        "taskId": task_id,
    });
    let mut stdout = io::stdout().lock();
    serde_json::to_writer(&mut stdout, &description).expect("serialize fixture description");
    writeln!(stdout).expect("write fixture description");
    stdout.flush().expect("flush fixture description");

    std::future::pending::<()>().await;
    shutdown.cancel();
    server.await.expect("coordinator server task");
}

async fn start_lossy_proxy(upstream: String) -> String {
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind lossy proxy");
    let address = listener.local_addr().expect("read lossy proxy address");
    tokio::spawn(serve_lossy_proxy(
        listener,
        upstream,
        Arc::new(AtomicBool::new(true)),
    ));
    format!("ws://{address}/connect")
}

async fn serve_lossy_proxy(
    listener: TcpListener,
    upstream: String,
    drop_acknowledgement: Arc<AtomicBool>,
) {
    while let Ok((client, _)) = listener.accept().await {
        tokio::spawn(proxy_connection(
            client,
            upstream.clone(),
            Arc::clone(&drop_acknowledgement),
        ));
    }
}

async fn proxy_connection(
    client: tokio::net::TcpStream,
    upstream: String,
    drop_acknowledgement: Arc<AtomicBool>,
) {
    let Ok(client) = accept_async(client).await else {
        return;
    };
    let Ok((upstream, _)) = connect_async(upstream).await else {
        return;
    };
    let (mut client_writer, mut client_reader) = client.split();
    let (mut upstream_writer, mut upstream_reader) = upstream.split();
    loop {
        tokio::select! {
            message = client_reader.next() => {
                let Some(Ok(message)) = message else { return };
                if upstream_writer.send(message).await.is_err() {
                    return;
                }
            }
            message = upstream_reader.next() => {
                let Some(Ok(message)) = message else { return };
                if is_execution_event_acknowledgement(&message)
                    && drop_acknowledgement.swap(false, Ordering::SeqCst)
                {
                    return;
                }
                if client_writer.send(message).await.is_err() {
                    return;
                }
            }
        }
    }
}

fn is_execution_event_acknowledgement(message: &Message) -> bool {
    let Message::Text(json) = message else {
        return false;
    };
    matches!(
        serde_json::from_str(json),
        Ok(ServerMessage::ExecutionEventsAccepted { .. })
    )
}

async fn start_coordinator(
    coordinator: Coordinator,
) -> (String, CancellationToken, tokio::task::JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind coordinator");
    let address = listener.local_addr().expect("read coordinator address");
    let endpoint = format!("ws://{address}/connect");
    let shutdown = CancellationToken::new();
    let server = tokio::spawn({
        let shutdown = shutdown.clone();
        async move {
            coordinator
                .serve(listener, shutdown)
                .await
                .expect("serve coordinator");
        }
    });
    (endpoint, shutdown, server)
}

async fn enroll(
    coordinator: &Coordinator,
    endpoint: &str,
    peer: PeerIdentity,
) -> DeviceCredentials {
    let token = coordinator
        .create_enrollment(peer, SystemTime::now() + Duration::from_mins(1))
        .await
        .expect("create enrollment");
    let (mut socket, _) = connect_async(endpoint)
        .await
        .expect("connect for enrollment");
    send(
        &mut socket,
        &ClientMessage::Enroll {
            version: JSON_WS_VERSION,
            token,
        },
    )
    .await;
    let ServerMessage::Enrolled { credentials, .. } = receive(&mut socket).await else {
        panic!("coordinator did not enroll fixture peer");
    };
    credentials
}

async fn send(socket: &mut Socket, message: &ClientMessage) {
    socket
        .send(Message::Text(
            serde_json::to_string(message)
                .expect("serialize client message")
                .into(),
        ))
        .await
        .expect("send client message");
}

async fn receive(socket: &mut Socket) -> ServerMessage {
    let message = socket
        .next()
        .await
        .expect("server closed connection")
        .expect("receive server message");
    let Message::Text(json) = message else {
        panic!("expected text frame");
    };
    serde_json::from_str(&json).expect("deserialize server message")
}
