use std::time::{Duration, SystemTime};

use futures_util::{SinkExt as _, StreamExt as _};
use renoa_control::{Coordinator, PeerIdentity};
use renoa_protocol::{
    CommandId, CommandInput, ExecutionEvent, ExecutionEventId, ExecutionEventKind, ExecutionId,
    ExecutionTerminal, LiveActivity, PrincipalId, SurfaceRef, TargetRef,
};
use renoa_rcp_client::{ClientError, Connection, Events, SurfaceEvent};
use renoa_rcp_wire::{
    ClientMessage, DeviceCredential, DeviceCredentials, DeviceId, ErrorCode, JSON_WS_VERSION,
    NodeId, ServerMessage, TaskEvent, TaskEventKind, TaskId,
};
use tempfile::TempDir;
use tokio::{net::TcpListener, time::timeout};
use tokio_tungstenite::{
    MaybeTlsStream, WebSocketStream, accept_async, connect_async, tungstenite::Message,
};
use tokio_util::sync::CancellationToken;

type Socket = WebSocketStream<MaybeTlsStream<tokio::net::TcpStream>>;

const TARGET: &str = "agent:alpha";

#[tokio::test]
async fn a_surface_opens_a_task_submits_and_receives_its_execution_in_order() {
    let system = System::start().await;
    let mut node = system.node().await;
    let (surface, mut events) = system.surface().await;
    let task_id = system.open(&surface).await;

    assert_eq!(surface.attach(task_id, None).await, Ok(None));
    let command_id = CommandId::new();
    surface
        .submit(
            task_id,
            command_id,
            CommandInput::from_text("Summarize today."),
        )
        .await
        .expect("submit command");
    execute(&mut node, task_id, command_id).await;

    let kinds = [
        next(&mut events).await,
        next(&mut events).await,
        next(&mut events).await,
    ];
    assert!(matches!(
        &kinds[0].kind,
        TaskEventKind::CommandSubmitted { command } if command.command_id == command_id
    ));
    assert!(matches!(
        &kinds[1].kind,
        TaskEventKind::ExecutionEvent { event, .. }
            if event.kind == ExecutionEventKind::ExecutionStarted
    ));
    assert!(matches!(
        &kinds[2].kind,
        TaskEventKind::ExecutionEvent { event, .. }
            if matches!(event.kind, ExecutionEventKind::ExecutionTerminated {
                terminal: ExecutionTerminal::Completed
            })
    ));
    assert_eq!(
        kinds.iter().map(|event| event.sequence).collect::<Vec<_>>(),
        vec![0, 1, 2]
    );

    let (reattached, mut replay) = system.surface().await;
    assert_eq!(reattached.attach(task_id, Some(0)).await, Ok(Some(2)));
    assert_eq!(next(&mut replay).await.sequence, 1);
    assert_eq!(next(&mut replay).await.sequence, 2);
    system.stop().await;
}

#[tokio::test]
async fn refusals_carry_the_coordinator_error_code() {
    let system = System::start().await;
    let node = system.node().await;
    let (surface, _events) = system.surface().await;
    let task_id = system.open(&surface).await;
    drop(node);
    wait_for_targets(&surface, 0).await;

    let refused = surface
        .submit(
            task_id,
            CommandId::new(),
            CommandInput::from_text("Are you there?"),
        )
        .await
        .expect_err("an offline node refuses new work");
    assert_eq!(refused.code(), Some(ErrorCode::NodeOffline));
    let missing = surface
        .attach(TaskId::new(), None)
        .await
        .expect_err("an unknown task is refused");
    assert_eq!(missing.code(), Some(ErrorCode::NotFound));
    system.stop().await;
}

#[tokio::test]
async fn a_closed_connection_ends_events_and_fails_later_requests() {
    let system = System::start().await;
    let (surface, mut events, device_id) = system.surface_device().await;

    system
        .coordinator
        .revoke_device(device_id)
        .await
        .expect("revoke the surface device");

    let ended = timeout(Duration::from_secs(5), events.next())
        .await
        .expect("the event stream reports the end");
    assert!(matches!(ended, Some(Err(ClientError::Transport(_)))));
    assert!(matches!(
        surface.list_targets().await,
        Err(ClientError::Transport(_))
    ));
    system.stop().await;
}

#[tokio::test]
async fn attach_live_delivers_live_activity_alongside_task_events() {
    let system = System::start().await;
    let mut node = system.node().await;
    let (surface, mut events) = system.surface().await;
    let task_id = system.open(&surface).await;

    assert_eq!(surface.attach_live(task_id, None).await, Ok(None));
    let command_id = CommandId::new();
    surface
        .submit(
            task_id,
            command_id,
            CommandInput::from_text("Check status."),
        )
        .await
        .expect("submit command");

    let ServerMessage::Execute { command, .. } = receive(&mut node).await else {
        panic!("node should receive execute");
    };
    assert_eq!(command.command_id, command_id);
    send(
        &mut node,
        &ClientMessage::AcknowledgeExecution {
            task_id,
            command_id,
        },
    )
    .await;
    assert_eq!(
        receive(&mut node).await,
        ServerMessage::ExecutionAcknowledged { command_id }
    );
    send(
        &mut node,
        &ClientMessage::PublishLiveActivity {
            task_id,
            command_id,
            activity: LiveActivity::Activity {
                text: "Scanning workspace".to_owned(),
            },
        },
    )
    .await;
    publish_events(
        &mut node,
        task_id,
        command_id,
        vec![
            ExecutionEventKind::ExecutionStarted,
            ExecutionEventKind::ToolStarted {
                call_id: "c1".to_owned(),
                name: "read_file".to_owned(),
                arguments: serde_json::json!({ "path": "Cargo.toml" }),
                activity: Some("Reading cargo manifest".to_owned()),
            },
            ExecutionEventKind::ExecutionTerminated {
                terminal: ExecutionTerminal::Completed,
            },
        ],
    )
    .await;

    let mut saw_live_activity = false;
    let mut saw_tool_activity = false;
    let mut saw_terminal = false;
    for _ in 0..10 {
        let item = timeout(Duration::from_secs(5), events.next_event())
            .await
            .expect("surface event arrives")
            .expect("stream open")
            .expect("no client error");
        match item {
            SurfaceEvent::Live {
                task_id: live_task,
                command_id: live_cmd,
                activity: LiveActivity::Activity { text },
            } => {
                assert_eq!(live_task, task_id);
                assert_eq!(live_cmd, command_id);
                assert_eq!(text, "Scanning workspace");
                saw_live_activity = true;
            }
            SurfaceEvent::Live { .. } => {}
            SurfaceEvent::Task(event) => match event.kind {
                TaskEventKind::ExecutionEvent { event, .. } => match event.kind {
                    ExecutionEventKind::ToolStarted { activity, .. } => {
                        assert_eq!(activity.as_deref(), Some("Reading cargo manifest"));
                        saw_tool_activity = true;
                    }
                    ExecutionEventKind::ExecutionTerminated { .. } => {
                        saw_terminal = true;
                        break;
                    }
                    _ => {}
                },
                TaskEventKind::CommandSubmitted { .. } => {}
            },
        }
    }
    assert!(saw_live_activity && saw_tool_activity && saw_terminal);
    system.stop().await;
}

#[tokio::test]
async fn unsolicited_live_activity_without_attach_live_fails_with_protocol_error() {
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind fake coordinator");
    let url = format!("ws://{}/connect", listener.local_addr().expect("addr"));
    let task_id = TaskId::new();
    let command_id = CommandId::new();

    let server = tokio::spawn(async move {
        let (stream, _) = listener.accept().await.expect("accept ws");
        let mut ws = accept_async(stream).await.expect("ws handshake");
        let _ = ws.next().await;
        let auth = serde_json::to_string(&ServerMessage::Authenticated {
            version: JSON_WS_VERSION,
        })
        .unwrap();
        ws.send(Message::Text(auth.into())).await.unwrap();

        let attach_raw = match ws.next().await.unwrap().unwrap() {
            Message::Text(text) => text,
            other => panic!("unexpected frame: {other:?}"),
        };
        let ClientMessage::Attach {
            request_id, live, ..
        } = serde_json::from_str(&attach_raw).unwrap()
        else {
            panic!("expected attach");
        };
        assert!(!live);
        let attached = serde_json::to_string(&ServerMessage::Attached {
            request_id,
            task_id,
            through_sequence: None,
        })
        .unwrap();
        ws.send(Message::Text(attached.into())).await.unwrap();

        let live_msg = serde_json::to_string(&ServerMessage::LiveActivity {
            task_id,
            command_id,
            activity: LiveActivity::Activity {
                text: "Unsolicited".to_owned(),
            },
        })
        .unwrap();
        ws.send(Message::Text(live_msg.into())).await.unwrap();
        tokio::time::sleep(Duration::from_millis(50)).await;
    });

    let credentials = DeviceCredentials {
        device_id: DeviceId::new(),
        credential: DeviceCredential::new("secret"),
    };
    let (connection, mut events) = renoa_rcp_client::connect(&url, credentials)
        .await
        .expect("connect surface");
    assert_eq!(connection.attach(task_id, None).await, Ok(None));

    let err = timeout(Duration::from_secs(5), events.next_event())
        .await
        .expect("protocol error arrives")
        .expect("stream item")
        .expect_err("unsolicited live activity must error");
    assert!(matches!(err, ClientError::Protocol(_)), "{err:?}");
    server.await.expect("fake coordinator finished");
}

struct System {
    _files: TempDir,
    coordinator: Coordinator,
    url: String,
    node_id: NodeId,
    principal_id: PrincipalId,
    shutdown: CancellationToken,
    server: tokio::task::JoinHandle<()>,
}

impl System {
    async fn start() -> Self {
        let files = TempDir::new().expect("temporary directory");
        let coordinator =
            Coordinator::open(files.path().join("control.sqlite")).expect("open coordinator");
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind coordinator");
        let url = format!("ws://{}/connect", listener.local_addr().expect("address"));
        let shutdown = CancellationToken::new();
        let serving = coordinator.clone();
        let stop = shutdown.clone();
        let server = tokio::spawn(async move {
            serving
                .serve(listener, stop)
                .await
                .expect("serve coordinator");
        });
        Self {
            _files: files,
            coordinator,
            url,
            node_id: NodeId::new(),
            principal_id: PrincipalId::new(),
            shutdown,
            server,
        }
    }

    async fn surface(&self) -> (Connection, Events) {
        let (connection, events, _) = self.surface_device().await;
        (connection, events)
    }

    async fn surface_device(&self) -> (Connection, Events, DeviceId) {
        let token = self
            .coordinator
            .create_enrollment(
                PeerIdentity::Surface {
                    principal_id: self.principal_id,
                    surface: SurfaceRef::new("discord"),
                },
                SystemTime::now() + Duration::from_mins(1),
            )
            .await
            .expect("create surface enrollment");
        let credentials = renoa_rcp_client::enroll(&self.url, token)
            .await
            .expect("enroll surface");
        let device_id = credentials.device_id;
        let (connection, events) = renoa_rcp_client::connect(&self.url, credentials)
            .await
            .expect("connect surface");
        (connection, events, device_id)
    }

    /// Connects a node that advertises one target, then waits for it to show.
    async fn node(&self) -> Socket {
        let token = self
            .coordinator
            .create_node_enrollment(
                self.node_id,
                self.principal_id,
                SystemTime::now() + Duration::from_mins(1),
            )
            .await
            .expect("create node enrollment");
        let credentials = renoa_rcp_client::enroll(&self.url, token)
            .await
            .expect("enroll node");
        let mut node = authenticate(&self.url, credentials).await;
        send(
            &mut node,
            &ClientMessage::AdvertiseTargets {
                targets: vec![renoa_protocol::TargetCard {
                    target: TargetRef::new(TARGET),
                    name: "Desk".to_owned(),
                }],
            },
        )
        .await;
        node
    }

    async fn open(&self, surface: &Connection) -> TaskId {
        wait_for_targets(surface, 1).await;
        let task_id = TaskId::new();
        surface
            .open_task(task_id, self.node_id, TargetRef::new(TARGET))
            .await
            .expect("open task");
        task_id
    }

    async fn stop(self) {
        self.shutdown.cancel();
        self.server.await.expect("coordinator task");
    }
}

async fn wait_for_targets(surface: &Connection, expected: usize) {
    for _ in 0..250 {
        if surface.list_targets().await.expect("list targets").len() == expected {
            return;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    panic!("expected {expected} advertised targets");
}

async fn execute(node: &mut Socket, task_id: TaskId, command_id: CommandId) {
    let ServerMessage::Execute { command, .. } = receive(node).await else {
        panic!("the node should receive the command");
    };
    assert_eq!(command.command_id, command_id);
    send(
        node,
        &ClientMessage::AcknowledgeExecution {
            task_id,
            command_id,
        },
    )
    .await;
    assert_eq!(
        receive(node).await,
        ServerMessage::ExecutionAcknowledged { command_id }
    );
    publish_events(
        node,
        task_id,
        command_id,
        vec![
            ExecutionEventKind::ExecutionStarted,
            ExecutionEventKind::ExecutionTerminated {
                terminal: ExecutionTerminal::Completed,
            },
        ],
    )
    .await;
}

async fn publish_events(
    node: &mut Socket,
    task_id: TaskId,
    command_id: CommandId,
    kinds: Vec<ExecutionEventKind>,
) {
    let execution_id = ExecutionId::new();
    send(
        node,
        &ClientMessage::PublishExecutionEvents {
            task_id,
            command_id,
            events: kinds
                .into_iter()
                .zip(0..)
                .map(|(kind, sequence)| ExecutionEvent {
                    event_id: ExecutionEventId::new(),
                    execution_id,
                    sequence,
                    recorded_at_ms: 0,
                    kind,
                })
                .collect(),
        },
    )
    .await;
    assert!(matches!(
        receive(node).await,
        ServerMessage::ExecutionEventsAccepted { .. }
    ));
}

async fn next(events: &mut Events) -> TaskEvent {
    timeout(Duration::from_secs(5), events.next())
        .await
        .expect("a task event arrives")
        .expect("the connection is open")
        .expect("a task event, not an error")
}

async fn authenticate(url: &str, credentials: DeviceCredentials) -> Socket {
    let (mut socket, _) = connect_async(url).await.expect("connect node");
    send(
        &mut socket,
        &ClientMessage::Authenticate {
            version: JSON_WS_VERSION,
            credentials,
        },
    )
    .await;
    assert!(matches!(
        receive(&mut socket).await,
        ServerMessage::Authenticated { .. }
    ));
    socket
}

async fn send(socket: &mut Socket, message: &ClientMessage) {
    let json = serde_json::to_string(message).expect("encode message");
    socket
        .send(Message::Text(json.into()))
        .await
        .expect("send message");
}

async fn receive(socket: &mut Socket) -> ServerMessage {
    loop {
        match socket.next().await.expect("server message").expect("frame") {
            Message::Text(json) => return serde_json::from_str(&json).expect("decode message"),
            Message::Ping(_) | Message::Pong(_) => {}
            other => panic!("unexpected frame {other:?}"),
        }
    }
}
