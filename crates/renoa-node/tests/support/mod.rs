use std::{
    fs,
    path::PathBuf,
    sync::Arc,
    time::{Duration, SystemTime},
};

use futures_util::{SinkExt, StreamExt};
use renoa_control::{
    ClientMessage, Coordinator, DeviceCredentials, EnrollmentToken, ErrorCode, JSON_WS_VERSION,
    NodeId, PeerIdentity, ServerMessage, StopOutcome, TargetSummary, TaskEvent, TaskEventKind,
    TaskId, TaskSpec, TaskSummary,
};
use renoa_kernel::{AgentId, EventCursor, Kernel, OperationStatus, SessionId};
use renoa_local::{
    AgentCreateRequest, AgentCreationOrigin, AgentCreator, AgentDocuments, AgentPresetId,
    LocalHost, LocalHostAdapters, LocalModelConfiguration, ModelProvider,
};
use renoa_protocol::{
    Author, CommandId, CommandInput, ExecutionEvent, ExecutionEventKind, LiveActivity, PrincipalId,
    SurfaceRef, TargetRef,
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
        Self::start_with(|database| Coordinator::open(database).expect("open coordinator store"))
            .await
    }

    /// A coordinator that also serves the browser identity and Host control
    /// routes for browsers at `origin`.
    pub(crate) async fn start_with_browsers(origin: &str) -> Self {
        Self::start_with(|database| {
            Coordinator::open_with_passkeys(database, "localhost", origin)
                .expect("open coordinator store")
        })
        .await
    }

    async fn start_with(open: impl FnOnce(PathBuf) -> Coordinator) -> Self {
        let files = TempDir::new().expect("temporary directory");
        let coordinator = open(files.path().join("control.sqlite"));
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

    /// The coordinator's address for plain HTTP requests.
    pub(crate) fn http_url(&self) -> String {
        self.url
            .replace("ws://", "http://")
            .trim_end_matches("/connect")
            .to_owned()
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
        let agent_id = provision_agent_named(&data, &bridge, &credentials, "Alpha").await;
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
        self.provision_agent_named("Alpha").await
    }

    /// Provisions another agent in the same Host under `name`.
    pub(crate) async fn provision_agent_named(&self, name: &str) -> AgentId {
        provision_agent_named(&self.data, &self.bridge, &self.credentials, name).await
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

    /// The process id the "Hold through reconnect." model wrote when it started.
    pub(crate) fn model_pid(&self) -> u32 {
        fs::read_to_string(self.workspace.join("model-pid"))
            .expect("read the model process id")
            .parse()
            .expect("parse the model process id")
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

    /// How many times the fixture model was asked for a context summary.
    pub(crate) fn summary_attempts(&self) -> usize {
        fs::read_to_string(self.workspace.join("summary-attempts")).map_or(0, |attempts| {
            attempts.parse().expect("summary attempt count")
        })
    }

    /// The last summary request the fixture model received.
    pub(crate) fn summary_request(&self) -> String {
        fs::read_to_string(self.workspace.join("summary-request.json"))
            .expect("read the summary request")
    }

    /// The last request of any kind the fixture model received, if any.
    pub(crate) fn last_request(&self) -> Option<String> {
        fs::read_to_string(self.workspace.join("last-request.json")).ok()
    }

    /// How many semantic events of `kind` the task's Host session holds.
    pub(crate) fn session_event_count(&self, task_id: TaskId, kind: &str) -> usize {
        let session_id = self.session_for(task_id);
        Kernel::open(
            self.data
                .join("sessions")
                .join(session_id.to_string())
                .join("kernel.sqlite3"),
        )
        .expect("open Host kernel")
        .events_after(SessionId::from_uuid(session_id), EventCursor::START)
        .expect("read Host session events")
        .events
        .iter()
        .filter(|event| event.kind == kind)
        .count()
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

    /// Records a stop in the node ledger for a command the node has not seen,
    /// as a stop delivered ahead of its command would.
    pub(crate) fn record_stop_in_ledger(&self, task_id: TaskId, command_id: CommandId) {
        rusqlite::Connection::open(&self.ledger)
            .expect("open node ledger")
            .execute(
                "INSERT INTO host_node_stops (command_id, task_id) VALUES (?1, ?2)",
                [command_id.to_string(), task_id.to_string()],
            )
            .expect("record the stop");
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

    /// How each kernel operation of the task's Host session ended, in order.
    pub(crate) fn operation_statuses_for(&self, task_id: TaskId) -> Vec<OperationStatus> {
        let session_id = self.session_for(task_id);
        Kernel::open(
            self.data
                .join("sessions")
                .join(session_id.to_string())
                .join("kernel.sqlite3"),
        )
        .expect("open Host kernel")
        .inspect(SessionId::from_uuid(session_id))
        .expect("inspect Host session")
        .operations
        .iter()
        .map(|operation| operation.status)
        .collect()
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

/// Provisions an agent built on the Alpha preset that a node fixture executes.
///
/// Node targets never create agents, so the durable definition must already
/// exist in the Host data root the node opens.
async fn provision_agent_named(
    data: &std::path::Path,
    bridge: &std::path::Path,
    credentials: &std::path::Path,
    name: &str,
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
            name,
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
            live: false,
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

/// Attaches to `task_id` asking for the live activity of its running turns.
pub(crate) async fn attach_live(socket: &mut Socket, task_id: TaskId) {
    send(
        socket,
        &ClientMessage::Attach {
            request_id: 1,
            task_id,
            after_sequence: None,
            live: true,
        },
    )
    .await;
    assert!(matches!(
        receive(socket).await,
        ServerMessage::Attached { request_id: 1, .. }
    ));
}

/// Asks to stop `command_id` from a fresh socket, so the answer never
/// interleaves with the events of an attached one.
pub(crate) async fn stop_command(
    system: &TestSystem,
    task_id: TaskId,
    command_id: CommandId,
) -> StopOutcome {
    let mut socket = system.connect_surface().await;
    send(
        &mut socket,
        &ClientMessage::StopCommand {
            request_id: 3,
            task_id,
            command_id,
        },
    )
    .await;
    let ServerMessage::StopAccepted {
        request_id: 3,
        command_id: stopped,
        outcome,
    } = receive(&mut socket).await
    else {
        panic!("the stop should be accepted");
    };
    assert_eq!(stopped, command_id);
    outcome
}

/// Reads a live-attached surface until the text it was shown reaches `text`,
/// returning the frames and the journal events that arrived meanwhile.
pub(crate) async fn collect_live_until_text(
    socket: &mut Socket,
    text: &str,
) -> (Vec<LiveActivity>, Vec<TaskEvent>) {
    let mut frames = Vec::new();
    let mut events = Vec::new();
    let mut shown = String::new();
    while shown != text {
        match receive(socket).await {
            ServerMessage::LiveActivity { activity, .. } => {
                if let LiveActivity::TextDelta { text: delta, .. } = &activity {
                    shown.push_str(delta);
                }
                assert!(
                    text.starts_with(&shown),
                    "the surface was shown {shown:?}, which is not a prefix of {text:?}"
                );
                frames.push(activity);
            }
            ServerMessage::TaskEvent { event } => events.push(event),
            other => panic!("unexpected message {other:?}"),
        }
    }
    (frames, events)
}

/// Every task the system's principal owns, asked on a fresh socket so the
/// answer never interleaves with an attachment's records.
pub(crate) async fn list_tasks(system: &TestSystem) -> Vec<TaskSummary> {
    let mut socket = system.connect_surface().await;
    send(&mut socket, &ClientMessage::ListTasks { request_id: 700 }).await;
    let ServerMessage::TaskList { tasks, .. } = receive(&mut socket).await else {
        panic!("expected a task list");
    };
    tasks
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
            live: false,
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

pub(crate) async fn receive(socket: &mut Socket) -> ServerMessage {
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
