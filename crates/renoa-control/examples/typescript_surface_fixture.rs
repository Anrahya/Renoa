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
    ClientMessage, Coordinator, DeviceCredentials, ErrorCode, JSON_WS_VERSION, NodeId,
    PeerIdentity, ServerMessage, TaskId, TaskSpec,
};
use renoa_protocol::{CommandId, CommandInput, PrincipalId, SurfaceRef, TargetRef};
use serde_json::json;
use tokio::net::TcpListener;
use tokio_tungstenite::{
    MaybeTlsStream, WebSocketStream, accept_async, connect_async, tungstenite::Message,
};
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

type Socket = WebSocketStream<MaybeTlsStream<tokio::net::TcpStream>>;
type FixtureTask = (TaskId, TargetRef, NodeId);

#[derive(Clone, Copy)]
enum ProxyFault {
    DropCommandAcknowledgement,
    RequireReplayAfterAttachment,
}

#[tokio::main]
async fn main() {
    let database = std::env::args_os()
        .nth(1)
        .map(PathBuf::from)
        .expect("usage: typescript_surface_fixture <database-path>");
    let coordinator = Coordinator::open(database).expect("open coordinator");
    let principal_id = PrincipalId::from_uuid(Uuid::from_u128(1));
    let node_id = NodeId::from_uuid(Uuid::from_u128(2));
    let (tasks, unowned_task_id) = create_tasks(&coordinator, principal_id, node_id).await;
    let (endpoint, shutdown, server) = start_coordinator(coordinator.clone()).await;
    let credentials = enroll(
        &coordinator,
        &endpoint,
        PeerIdentity::Surface {
            principal_id,
            surface: SurfaceRef::new("typescript_test"),
        },
    )
    .await;
    let node_credentials = enroll(&coordinator, &endpoint, PeerIdentity::Node { node_id }).await;
    let mut node = authenticate(&endpoint, node_credentials).await;
    seed_journal(&coordinator, &endpoint, principal_id, tasks[0].0).await;
    let lossy_endpoint =
        start_fault_proxy(endpoint.clone(), ProxyFault::DropCommandAcknowledgement).await;
    let replay_endpoint =
        start_fault_proxy(endpoint.clone(), ProxyFault::RequireReplayAfterAttachment).await;
    write_description(
        &endpoint,
        &lossy_endpoint,
        &replay_endpoint,
        &credentials,
        &tasks,
        unowned_task_id,
    );

    while node.next().await.is_some() {}
    shutdown.cancel();
    server.await.expect("coordinator server task");
}

async fn create_tasks(
    coordinator: &Coordinator,
    principal_id: PrincipalId,
    node_id: NodeId,
) -> ([FixtureTask; 3], TaskId) {
    let offline_node_id = NodeId::from_uuid(Uuid::from_u128(4));
    let tasks = [
        (
            TaskId::from_uuid(Uuid::from_u128(16)),
            TargetRef::new("workspace:alpha"),
            node_id,
        ),
        (
            TaskId::from_uuid(Uuid::from_u128(17)),
            TargetRef::new("workspace:beta"),
            node_id,
        ),
        (
            TaskId::from_uuid(Uuid::from_u128(19)),
            TargetRef::new("workspace:offline"),
            offline_node_id,
        ),
    ];
    for (task_id, target, task_node_id) in &tasks {
        coordinator
            .create_task(TaskSpec {
                task_id: *task_id,
                principal_id,
                node_id: *task_node_id,
                target: target.clone(),
            })
            .await
            .expect("create owned task");
    }
    let unowned_task_id = TaskId::from_uuid(Uuid::from_u128(18));
    coordinator
        .create_task(TaskSpec {
            task_id: unowned_task_id,
            principal_id: PrincipalId::from_uuid(Uuid::from_u128(3)),
            node_id,
            target: TargetRef::new("workspace:not-owned"),
        })
        .await
        .expect("create unowned task");
    (tasks, unowned_task_id)
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

async fn seed_journal(
    coordinator: &Coordinator,
    endpoint: &str,
    principal_id: PrincipalId,
    task_id: TaskId,
) {
    let seed_credentials = enroll(
        coordinator,
        endpoint,
        PeerIdentity::Surface {
            principal_id,
            surface: SurfaceRef::new("fixture_seed"),
        },
    )
    .await;
    let mut seed_surface = authenticate(endpoint, seed_credentials).await;
    let seed_command_id = CommandId::from_uuid(Uuid::from_u128(256));
    send(
        &mut seed_surface,
        &ClientMessage::Submit {
            request_id: 1,
            task_id,
            command_id: seed_command_id,
            input: CommandInput::Text {
                text: "seed the replay journal".to_owned(),
            },
        },
    )
    .await;
    assert_eq!(
        receive(&mut seed_surface).await,
        ServerMessage::CommandAccepted {
            request_id: 1,
            command_id: seed_command_id,
        }
    );
    seed_surface.close(None).await.expect("close seed surface");
}

async fn start_fault_proxy(upstream: String, fault: ProxyFault) -> String {
    let proxy_listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind fault proxy");
    let proxy_address = proxy_listener.local_addr().expect("read proxy address");
    tokio::spawn(serve_fault_proxy(
        proxy_listener,
        upstream,
        fault,
        Arc::new(AtomicBool::new(true)),
    ));
    format!("ws://{proxy_address}/connect")
}

fn write_description(
    endpoint: &str,
    lossy_endpoint: &str,
    replay_endpoint: &str,
    credentials: &DeviceCredentials,
    tasks: &[FixtureTask; 3],
    unowned_task_id: TaskId,
) {
    let description = json!({
        "endpoint": endpoint,
        "lossyEndpoint": lossy_endpoint,
        "replayEndpoint": replay_endpoint,
        "credentials": credentials,
        "unownedTaskId": unowned_task_id,
        "tasks": tasks.iter().map(|(task_id, target, _)| json!({
            "taskId": task_id,
            "target": target,
        })).collect::<Vec<_>>(),
    });
    let mut stdout = io::stdout().lock();
    serde_json::to_writer(&mut stdout, &description).expect("serialize fixture description");
    writeln!(stdout).expect("write fixture description");
    stdout.flush().expect("flush fixture description");
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

async fn authenticate(endpoint: &str, credentials: DeviceCredentials) -> Socket {
    let (mut socket, _) = connect_async(endpoint).await.expect("connect node");
    send(
        &mut socket,
        &ClientMessage::Authenticate {
            version: JSON_WS_VERSION,
            credentials,
        },
    )
    .await;
    assert_eq!(
        receive(&mut socket).await,
        ServerMessage::Authenticated {
            version: JSON_WS_VERSION,
        }
    );
    socket
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

async fn serve_fault_proxy(
    listener: TcpListener,
    upstream: String,
    fault: ProxyFault,
    trigger_fault: Arc<AtomicBool>,
) {
    while let Ok((client, _)) = listener.accept().await {
        tokio::spawn(proxy_connection(
            client,
            upstream.clone(),
            fault,
            Arc::clone(&trigger_fault),
        ));
    }
}

async fn proxy_connection(
    client: tokio::net::TcpStream,
    upstream: String,
    fault: ProxyFault,
    trigger_fault: Arc<AtomicBool>,
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
                match fault {
                    ProxyFault::DropCommandAcknowledgement
                        if is_command_acknowledgement(&message)
                            && trigger_fault.swap(false, Ordering::SeqCst) => return,
                    ProxyFault::RequireReplayAfterAttachment
                        if is_attachment(&message)
                            && trigger_fault.swap(false, Ordering::SeqCst) => {
                            if client_writer.send(message).await.is_err() {
                                return;
                            }
                            let error = ServerMessage::Error {
                                request_id: None,
                                code: ErrorCode::ReplayRequired,
                                message: "surface fell behind; reconnect with its last task sequence"
                                    .to_owned(),
                            };
                            let error = Message::Text(
                                serde_json::to_string(&error)
                                    .expect("serialize replay-required error")
                                    .into(),
                            );
                            let _ = client_writer.send(error).await;
                            return;
                        }
                    _ => {}
                }
                if client_writer.send(message).await.is_err() {
                    return;
                }
            }
        }
    }
}

fn is_command_acknowledgement(message: &Message) -> bool {
    let Message::Text(json) = message else {
        return false;
    };
    matches!(
        serde_json::from_str(json),
        Ok(ServerMessage::CommandAccepted { .. })
    )
}

fn is_attachment(message: &Message) -> bool {
    let Message::Text(json) = message else {
        return false;
    };
    matches!(
        serde_json::from_str(json),
        Ok(ServerMessage::Attached { .. })
    )
}
