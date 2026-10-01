use std::{
    fs,
    path::PathBuf,
    sync::Arc,
    time::{Duration, SystemTime},
};

use futures_util::{SinkExt, StreamExt};
use renoa_control::{
    ClientMessage, Coordinator, DeviceCredentials, EnrollmentToken, ErrorCode, JSON_WS_VERSION,
    NodeId, PeerIdentity, ServerMessage, TargetSummary, TaskEvent, TaskEventKind, TaskId, TaskSpec,
};
use renoa_kernel::{AgentId, Kernel, SessionId};
use renoa_local::{
    AgentCreateRequest, AgentCreationOrigin, AgentCreator, AgentDocuments, AgentPresetId,
    LocalHost, LocalHostAdapters, LocalModelConfiguration, ModelProvider,
};
use renoa_protocol::{
    Author, CommandId, CommandInput, ExecutionEvent, ExecutionEventKind, PrincipalId, SurfaceRef,
    TargetRef,
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
        // `HostFixture::install` names the provisioned agent and creates the task.
        let target = TargetRef::new("agent:unprovisioned");
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

    /// Enrolls the system's node with the system's principal as its owner.
    pub(crate) async fn enroll_node(&self) -> DeviceCredentials {
        let token = self
            .coordinator
            .create_node_enrollment(
                self.node_id,
                self.principal_id,
                SystemTime::now() + Duration::from_mins(1),
            )
            .await
            .expect("create node enrollment");
        self.claim(token).await
    }

    pub(crate) const fn node_id(&self) -> NodeId {
        self.node_id
    }

    /// The principal that owns the system's node and submits its commands.
    pub(crate) const fn principal_id(&self) -> PrincipalId {
        self.principal_id
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
        self.claim(token).await
    }

    async fn claim(&self, token: EnrollmentToken) -> DeviceCredentials {
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
    /// Where the fixture model bridge records starts and waits for releases.
    pub(crate) workspace: PathBuf,
    pub(crate) agent_id: AgentId,
    bridge: PathBuf,
    credentials: PathBuf,
    task_id: TaskId,
    ledger: PathBuf,
}

impl HostFixture {
    /// Provisions Alpha, gives its Host workspace a proof file, and creates the
    /// system's task on Alpha's advertised target.
    pub(crate) async fn install(system: &mut TestSystem) -> Self {
        let data = system.files.path().join("host");
        let workspace = system.files.path().join("model-control");
        let bridge = system.files.path().join("model-bridge.mjs");
        let credentials = system.files.path().join("credentials.sqlite3");
        fs::create_dir(&workspace).expect("create model control directory");
        fs::write(&bridge, bridge_script(&workspace)).expect("write model bridge");
        fs::write(&credentials, "").expect("write credential placeholder");
        let agent_id = provision_alpha(&data, &bridge, &credentials).await;
        let fixture = Self {
            ledger: data.join("state/node.sqlite3"),
            data,
            workspace,
            agent_id,
            bridge,
            credentials,
            task_id: system.task_id,
        };
        let agent_workspace = fixture
            .host()
            .agent_workspace(agent_id)
            .await
            .expect("open Alpha's Host workspace");
        fs::write(agent_workspace.join("proof.txt"), "durable proof\n").expect("write proof file");
        system.target = agent_target(agent_id);
        system
            .coordinator
            .create_task(TaskSpec {
                task_id: system.task_id,
                principal_id: system.principal_id,
                node_id: system.node_id,
                target: system.target.clone(),
            })
            .await
            .expect("create task");
        fixture
    }

    /// Provisions another agent in the same Host.
    pub(crate) async fn provision_agent(&self) -> AgentId {
        provision_alpha(&self.data, &self.bridge, &self.credentials).await
    }

    /// Provisions an agent that reads the speaking person's `USER.md` and may
    /// read files, and records `profile` as that file for `principal`.
    pub(crate) async fn provision_profiled_agent(
        &self,
        principal: PrincipalId,
        profile: &str,
    ) -> AgentId {
        let directory = self
            .data
            .join("users")
            .join(principal.as_uuid().to_string());
        fs::create_dir_all(&directory).expect("create profile directory");
        fs::write(directory.join("USER.md"), profile).expect("write profile");
        let mut request = AgentCreateRequest::new(Uuid::new_v4(), "Profiled", "Answer the person.")
            .with_tools(["read_file".to_owned()]);
        request.documents = Some(AgentDocuments {
            soul: false,
            user: true,
        });
        self.host()
            .create_agent(
                AgentCreator::System {
                    component: "node-test".to_owned(),
                },
                AgentCreationOrigin::Provisioning,
                request,
                CancellationToken::new(),
            )
            .await
            .expect("provision profiled agent")
            .id
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
                LocalHostAdapters::default(),
            )
            .expect("assemble local Host"),
        )
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
        self.operation_count_for(self.task_id)
    }

    /// The Host session the node ledger recorded for the task's first command.
    pub(crate) fn session_for(&self, task_id: TaskId) -> Uuid {
        let session_id: String = rusqlite::Connection::open(&self.ledger)
            .expect("open node ledger")
            .query_row(
                "SELECT session_id FROM host_node_tasks WHERE task_id = ?1",
                [task_id.to_string()],
                |row| row.get(0),
            )
            .expect("read the task's Host session");
        session_id.parse().expect("stored session id")
    }

    /// Whether the node ledger still records `task_id`.
    pub(crate) fn ledger_has_task(&self, task_id: TaskId) -> bool {
        rusqlite::Connection::open(&self.ledger)
            .expect("open node ledger")
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM host_node_tasks WHERE task_id = ?1)",
                [task_id.to_string()],
                |row| row.get(0),
            )
            .expect("read the node ledger")
    }

    pub(crate) fn operation_count_for(&self, task_id: TaskId) -> usize {
        let session_id = self.session_for(task_id);
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

/// Provisions the canonical Alpha agent a node fixture executes.
///
/// Node targets never create agents, so the durable definition must already
/// exist in the Host data root the node opens.
async fn provision_alpha(
    data: &std::path::Path,
    bridge: &std::path::Path,
    credentials: &std::path::Path,
) -> AgentId {
    let host = LocalHost::new(
        data,
        LocalModelConfiguration::new(
            bridge,
            vec![ModelProvider::Xai],
            ModelProvider::Xai,
            "fixture-model",
            credentials,
        ),
        LocalHostAdapters::default(),
    )
    .expect("provisioning Host");
    host.create_agent(
        AgentCreator::System {
            component: "node-test".to_owned(),
        },
        AgentCreationOrigin::Provisioning,
        AgentCreateRequest::from_preset(
            Uuid::new_v4(),
            AgentPresetId::new("renoa.coding.alpha.v3").expect("alpha preset id"),
            "Alpha",
        ),
        CancellationToken::new(),
    )
    .await
    .expect("provision Alpha")
    .id
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

/// Whether the coordinator still holds `task_id`, asked on a fresh socket so
/// an attachment never interleaves with the caller's.
pub(crate) async fn coordinator_has_task(system: &TestSystem, task_id: TaskId) -> bool {
    let mut socket = system.connect_surface().await;
    send(
        &mut socket,
        &ClientMessage::Attach {
            request_id: 1,
            task_id,
            after_sequence: None,
        },
    )
    .await;
    match receive(&mut socket).await {
        ServerMessage::Attached { .. } => true,
        ServerMessage::Error {
            code: ErrorCode::NotFound,
            ..
        } => false,
        other => panic!("unexpected reply to attaching to {task_id}: {other:?}"),
    }
}

pub(crate) fn agent_target(agent_id: AgentId) -> TargetRef {
    TargetRef::new(format!("agent:{agent_id}"))
}

/// Polls until the node's advertisement reaches the coordinator.
pub(crate) async fn wait_for_targets(surface: &mut Socket, expected: usize) -> Vec<TargetSummary> {
    // The node polls its Host for agents every five seconds.
    for request_id in 900..1400 {
        send(surface, &ClientMessage::ListTargets { request_id }).await;
        let ServerMessage::TargetList { targets, .. } = receive(surface).await else {
            panic!("expected a target list");
        };
        if targets.len() == expected {
            return targets;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    panic!("expected {expected} advertised targets");
}

pub(crate) async fn open_task(surface: &mut Socket, node_id: NodeId, target: TargetRef) -> TaskId {
    let task_id = TaskId::new();
    send(
        surface,
        &ClientMessage::OpenTask {
            request_id: 800,
            task_id,
            node_id,
            target,
        },
    )
    .await;
    assert_eq!(
        receive(surface).await,
        ServerMessage::TaskOpened {
            request_id: 800,
            task_id,
        }
    );
    task_id
}

pub(crate) async fn submit_when_node_is_online(
    socket: &mut Socket,
    task_id: TaskId,
    command_id: CommandId,
    text: &str,
) {
    submit_placed_when_node_is_online(socket, task_id, command_id, text, None).await;
}

/// Submits text with the surface's description of where it was written.
pub(crate) async fn submit_placed_when_node_is_online(
    socket: &mut Socket,
    task_id: TaskId,
    command_id: CommandId,
    text: &str,
    context: Option<&str>,
) {
    submit_input_when_node_is_online(
        socket,
        task_id,
        command_id,
        CommandInput::Text {
            text: text.to_owned(),
            context: context.map(str::to_owned),
            author: Author::Principal,
        },
    )
    .await;
}

/// Submits one command input once its node is online.
pub(crate) async fn submit_input_when_node_is_online(
    socket: &mut Socket,
    task_id: TaskId,
    command_id: CommandId,
    input: CommandInput,
) {
    loop {
        send(
            socket,
            &ClientMessage::Submit {
                request_id: 2,
                task_id,
                command_id,
                input: input.clone(),
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

pub(crate) async fn collect_until(
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
