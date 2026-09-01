use std::{
    fs,
    path::PathBuf,
    sync::Arc,
    time::{Duration, SystemTime},
};

use futures_util::{SinkExt, StreamExt};
use renoa_control::{
    ClientMessage, Coordinator, DeviceCredentials, ErrorCode, JSON_WS_VERSION, NodeId,
    PeerIdentity, ServerMessage, TaskEvent, TaskEventKind, TaskId, TaskSpec,
};
use renoa_kernel::{Kernel, SessionId};
use renoa_local::{
    ALPHA_PROFILE_ID, AgentProfileId, LocalHost, LocalHostAdapters, LocalModelConfiguration,
    ModelProvider, alpha_profile,
};
use renoa_node::HostTarget;
use renoa_protocol::{
    CommandId, CommandInput, ExecutionEvent, ExecutionEventKind, PrincipalId, SurfaceRef, TargetRef,
};
use tempfile::TempDir;
use tokio::{
    net::TcpListener,
    sync::{mpsc, oneshot},
    task::JoinSet,
};
use tokio_tungstenite::{
    MaybeTlsStream, WebSocketStream, accept_async, connect_async, tungstenite::Message,
};
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

mod model_bridge;

use model_bridge::bridge_script;
pub(crate) use model_bridge::wait_for_path;

pub(crate) type Socket = WebSocketStream<MaybeTlsStream<tokio::net::TcpStream>>;

pub(crate) struct TestSystem {
    pub(crate) files: TempDir,
    pub(crate) coordinator: Coordinator,
    pub(crate) url: String,
    pub(crate) task_id: TaskId,
    pub(crate) target: TargetRef,
    node_id: NodeId,
    principal_id: PrincipalId,
    shutdown: CancellationToken,
    server: Option<tokio::task::JoinHandle<()>>,
}

impl TestSystem {
    pub(crate) async fn start() -> Self {
        let files = TempDir::new().expect("temporary directory");
        let coordinator =
            Coordinator::open(files.path().join("control.sqlite")).expect("open coordinator store");
        let task_id = TaskId::new();
        let node_id = NodeId::new();
        let principal_id = PrincipalId::new();
        let target = TargetRef::new("workspace:live");
        coordinator
            .create_task(TaskSpec {
                task_id,
                principal_id,
                node_id,
                target: target.clone(),
            })
            .await
            .expect("create task");
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind coordinator");
        let address = listener.local_addr().expect("coordinator address");
        let shutdown = CancellationToken::new();
        let server = spawn_server(coordinator.clone(), listener, shutdown.clone());
        Self {
            files,
            coordinator,
            url: format!("ws://{address}/connect"),
            task_id,
            target,
            node_id,
            principal_id,
            shutdown,
            server: Some(server),
        }
    }

    pub(crate) async fn enroll_node(&self) -> DeviceCredentials {
        self.enroll(PeerIdentity::Node {
            node_id: self.node_id,
        })
        .await
    }

    pub(crate) async fn create_task(&self, target: TargetRef) -> TaskId {
        let task_id = TaskId::new();
        self.coordinator
            .create_task(TaskSpec {
                task_id,
                principal_id: self.principal_id,
                node_id: self.node_id,
                target,
            })
            .await
            .expect("create additional task");
        task_id
    }

    pub(crate) async fn enroll_surface(&self) -> DeviceCredentials {
        self.enroll_surface_as("test_surface").await
    }

    pub(crate) async fn enroll_surface_as(&self, name: &str) -> DeviceCredentials {
        self.enroll(PeerIdentity::Surface {
            principal_id: self.principal_id,
            surface: SurfaceRef::new(name),
        })
        .await
    }

    pub(crate) async fn connect_surface(&self) -> Socket {
        let credentials = self.enroll_surface().await;
        self.connect(&credentials).await
    }

    pub(crate) async fn connect(&self, credentials: &DeviceCredentials) -> Socket {
        let (mut socket, _) = connect_async(&self.url).await.expect("connect device");
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

    pub(crate) async fn stop(mut self) {
        self.shutdown.cancel();
        self.server
            .take()
            .expect("running coordinator")
            .await
            .expect("coordinator task");
    }

    async fn enroll(&self, peer: PeerIdentity) -> DeviceCredentials {
        let token = self
            .coordinator
            .create_enrollment(peer, SystemTime::now() + Duration::from_mins(1))
            .await
            .expect("create enrollment");
        let (mut socket, _) = connect_async(&self.url).await.expect("connect enrollment");
        send(
            &mut socket,
            &ClientMessage::Enroll {
                version: JSON_WS_VERSION,
                token,
            },
        )
        .await;
        let ServerMessage::Enrolled { credentials, .. } = receive(&mut socket).await else {
            panic!("server should enroll device");
        };
        credentials
    }
}

pub(crate) struct HostFixture {
    pub(crate) data: PathBuf,
    pub(crate) workspace: PathBuf,
    bridge: PathBuf,
    credentials: PathBuf,
    target: TargetRef,
    pub(crate) session_id: Uuid,
}

impl HostFixture {
    pub(crate) fn install(system: &TestSystem) -> Self {
        let data = system.files.path().join("host");
        let workspace = system.files.path().join("workspace");
        let bridge = system.files.path().join("model-bridge.mjs");
        let credentials = system.files.path().join("credentials.sqlite3");
        fs::create_dir(&workspace).expect("create Host workspace");
        fs::write(workspace.join("proof.txt"), "durable proof\n").expect("write proof file");
        fs::write(&bridge, bridge_script(&workspace)).expect("write model bridge");
        fs::write(&credentials, "").expect("write credential placeholder");
        Self {
            data,
            workspace,
            bridge,
            credentials,
            target: system.target.clone(),
            session_id: Uuid::new_v4(),
        }
    }

    pub(crate) fn host(&self) -> Arc<LocalHost> {
        Arc::new(
            LocalHost::new(
                &self.data,
                LocalModelConfiguration::new(
                    &self.bridge,
                    vec![ModelProvider::Xai],
                    ModelProvider::Xai,
                    "fixture-model",
                    &self.credentials,
                ),
                vec![alpha_profile()],
                LocalHostAdapters::default(),
            )
            .expect("assemble local Host"),
        )
    }

    pub(crate) fn target(&self) -> HostTarget {
        Self::target_for(&self.target, self.session_id, &self.workspace)
    }

    pub(crate) fn target_for(
        target: &TargetRef,
        session_id: Uuid,
        workspace: &std::path::Path,
    ) -> HostTarget {
        HostTarget::new(
            target,
            AgentProfileId::new(ALPHA_PROFILE_ID).expect("valid Alpha profile id"),
            session_id,
            workspace,
        )
        .expect("configure Host target")
    }

    pub(crate) fn additional_workspace(&self) -> PathBuf {
        let workspace = self.workspace.with_file_name("workspace-two");
        fs::create_dir(&workspace).expect("create second Host workspace");
        workspace
    }

    pub(crate) fn started(&self) -> PathBuf {
        self.workspace.join("model-started")
    }

    pub(crate) fn release(&self) {
        fs::write(self.workspace.join("model-release"), "release").expect("release model");
    }

    pub(crate) fn attempts(&self) -> String {
        fs::read_to_string(self.workspace.join("model-attempts")).expect("read model attempts")
    }

    pub(crate) fn operation_count(&self) -> usize {
        self.operation_count_for(self.session_id)
    }

    pub(crate) fn operation_count_for(&self, session_id: Uuid) -> usize {
        let session = SessionId::from_uuid(session_id);
        Kernel::open(
            self.data
                .join("sessions")
                .join(session_id.to_string())
                .join("kernel.sqlite3"),
        )
        .expect("open Host kernel")
        .inspect(session)
        .expect("inspect Host session")
        .operations
        .len()
    }
}

pub(crate) struct CuttableProxy {
    pub(crate) url: String,
    cuts: mpsc::Sender<oneshot::Sender<()>>,
    shutdown: CancellationToken,
    task: tokio::task::JoinHandle<()>,
}

impl CuttableProxy {
    pub(crate) async fn start(upstream: String) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind WebSocket proxy");
        let address = listener.local_addr().expect("proxy address");
        let (cuts, mut cut_requests) = mpsc::channel::<oneshot::Sender<()>>(1);
        let shutdown = CancellationToken::new();
        let task_shutdown = shutdown.clone();
        let task = tokio::spawn(async move {
            let mut connections = JoinSet::new();
            loop {
                tokio::select! {
                    () = task_shutdown.cancelled() => break,
                    request = cut_requests.recv() => {
                        let Some(request) = request else { break };
                        connections.abort_all();
                        while connections.join_next().await.is_some() {}
                        let _ = request.send(());
                    }
                    accepted = listener.accept() => {
                        let Ok((client, _)) = accepted else { break };
                        connections.spawn(proxy_connection(client, upstream.clone()));
                    }
                    _ = connections.join_next(), if !connections.is_empty() => {}
                }
            }
            connections.abort_all();
            while connections.join_next().await.is_some() {}
        });
        Self {
            url: format!("ws://{address}/connect"),
            cuts,
            shutdown,
            task,
        }
    }

    pub(crate) async fn cut(&self) {
        let (completed, completion) = oneshot::channel();
        self.cuts.send(completed).await.expect("request proxy cut");
        completion.await.expect("proxy cut completes");
    }

    pub(crate) async fn stop(self) {
        self.shutdown.cancel();
        self.task.await.expect("proxy task");
    }
}

async fn proxy_connection(client: tokio::net::TcpStream, upstream: String) {
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
                if upstream_writer.send(message).await.is_err() { return; }
            }
            message = upstream_reader.next() => {
                let Some(Ok(message)) = message else { return };
                if client_writer.send(message).await.is_err() { return; }
            }
        }
    }
}

fn spawn_server(
    coordinator: Coordinator,
    listener: TcpListener,
    shutdown: CancellationToken,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        coordinator
            .serve(listener, shutdown)
            .await
            .expect("serve coordinator");
    })
}

pub(crate) async fn attach(socket: &mut Socket, task_id: TaskId) {
    assert_eq!(attach_after(socket, task_id, None).await, None);
}

pub(crate) async fn attach_after(
    socket: &mut Socket,
    task_id: TaskId,
    after_sequence: Option<u64>,
) -> Option<u64> {
    send(
        socket,
        &ClientMessage::Attach {
            request_id: 1,
            task_id,
            after_sequence,
        },
    )
    .await;
    let ServerMessage::Attached {
        request_id: 1,
        task_id: attached_task,
        through_sequence,
    } = receive(socket).await
    else {
        panic!("surface should attach");
    };
    assert_eq!(attached_task, task_id);
    through_sequence
}

pub(crate) async fn submit_when_node_is_online(
    socket: &mut Socket,
    task_id: TaskId,
    command_id: CommandId,
    text: &str,
) {
    loop {
        send(
            socket,
            &ClientMessage::Submit {
                request_id: 2,
                task_id,
                command_id,
                input: CommandInput::Text {
                    text: text.to_owned(),
                },
            },
        )
        .await;
        match receive(socket).await {
            ServerMessage::CommandAccepted {
                request_id: 2,
                command_id: accepted,
            } if accepted == command_id => return,
            ServerMessage::Error {
                code: ErrorCode::NodeOffline,
                ..
            } => tokio::task::yield_now().await,
            message => panic!("unexpected submission response: {message:?}"),
        }
    }
}

pub(crate) async fn collect_through_turn_started(socket: &mut Socket) -> Vec<TaskEvent> {
    collect_until(socket, |event| {
        matches!(event.kind, ExecutionEventKind::TurnStarted)
    })
    .await
}

pub(crate) async fn collect_through_terminal(socket: &mut Socket) -> Vec<TaskEvent> {
    collect_until(socket, |event| {
        matches!(event.kind, ExecutionEventKind::ExecutionTerminated { .. })
    })
    .await
}

async fn collect_until(
    socket: &mut Socket,
    complete: impl Fn(&ExecutionEvent) -> bool,
) -> Vec<TaskEvent> {
    let mut events = Vec::new();
    loop {
        let ServerMessage::TaskEvent { event } = receive(socket).await else {
            continue;
        };
        let done = match &event.kind {
            TaskEventKind::ExecutionEvent { event, .. } => complete(event),
            TaskEventKind::CommandSubmitted { .. } => false,
        };
        events.push(event);
        if done {
            return events;
        }
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
    let message = socket
        .next()
        .await
        .expect("server message")
        .expect("valid websocket message");
    let Message::Text(json) = message else {
        panic!("expected text websocket message")
    };
    serde_json::from_str(&json).expect("deserialize server message")
}
