use std::time::{Duration, SystemTime};

use futures_util::{SinkExt, StreamExt};
use renoa_control::{
    ClientMessage, Coordinator, DeviceCredentials, ErrorCode, JSON_WS_VERSION, NodeId,
    PeerIdentity, ServerMessage, TaskEvent, TaskEventKind, TaskId, TaskSpec,
};
use renoa_core::{
    BoxFuture, CapabilityHost, CapabilityOutcome, CapabilityRequest, CommandId, CommandInput,
    Message as AgentMessage, ModelDriver, ModelError, ModelRequest, ModelResponse, PrincipalId,
    ResolvedAgent, SurfaceRef, TargetRef,
};
use renoa_protocol::{ExecutionEvent, ExecutionEventKind};
use tempfile::TempDir;
use tokio::{
    net::TcpListener,
    sync::{Semaphore, mpsc, oneshot},
    task::JoinSet,
};
use tokio_tungstenite::{
    MaybeTlsStream, WebSocketStream, accept_async, connect_async, tungstenite::Message,
};
use tokio_util::sync::CancellationToken;

pub(crate) type Socket = WebSocketStream<MaybeTlsStream<tokio::net::TcpStream>>;

const TEST_INSTRUCTIONS: &str = "Complete the live bridge test.";

pub(crate) struct TestSystem {
    pub(crate) files: TempDir,
    pub(crate) coordinator: Coordinator,
    pub(crate) url: String,
    pub(crate) task_id: TaskId,
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
        coordinator
            .create_task(TaskSpec {
                task_id,
                principal_id,
                node_id,
                target: TargetRef::new("workspace:live"),
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

    pub(crate) async fn enroll_surface(&self) -> DeviceCredentials {
        self.enroll(PeerIdentity::Surface {
            principal_id: self.principal_id,
            surface: SurfaceRef::new("test_surface"),
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
                if upstream_writer.send(message).await.is_err() {
                    return;
                }
            }
            message = upstream_reader.next() => {
                let Some(Ok(message)) = message else { return };
                if client_writer.send(message).await.is_err() {
                    return;
                }
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

pub(crate) struct GatedModel {
    requested: Semaphore,
    release: Semaphore,
}

impl GatedModel {
    pub(crate) fn new() -> Self {
        Self {
            requested: Semaphore::new(0),
            release: Semaphore::new(0),
        }
    }

    pub(crate) async fn wait_until_requested(&self) {
        self.requested
            .acquire()
            .await
            .expect("request semaphore")
            .forget();
    }

    pub(crate) fn release(&self) {
        self.release.add_permits(1);
    }
}

impl ModelDriver for GatedModel {
    fn generate(
        &self,
        request: ModelRequest,
        _cancellation: CancellationToken,
    ) -> BoxFuture<'_, Result<ModelResponse, ModelError>> {
        assert!(matches!(
            request.messages.first(),
            Some(AgentMessage::System { text }) if text == TEST_INSTRUCTIONS
        ));
        Box::pin(async move {
            self.requested.add_permits(1);
            self.release
                .acquire()
                .await
                .expect("release semaphore")
                .forget();
            Ok(ModelResponse {
                text: "finished live".to_owned(),
                capability_calls: Vec::new(),
                truncated: false,
            })
        })
    }
}

pub(crate) fn test_agent() -> ResolvedAgent {
    ResolvedAgent {
        instructions: TEST_INSTRUCTIONS.to_owned(),
        capability_grants: Vec::new(),
    }
}

pub(crate) struct NoCapabilities;

impl CapabilityHost for NoCapabilities {
    fn specs(&self) -> Vec<renoa_core::CapabilitySpec> {
        Vec::new()
    }

    fn execute(
        &self,
        _request: CapabilityRequest,
        _cancellation: CancellationToken,
    ) -> BoxFuture<'_, CapabilityOutcome> {
        Box::pin(async { CapabilityOutcome::error("no capabilities") })
    }
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
) {
    loop {
        send(
            socket,
            &ClientMessage::Submit {
                request_id: 2,
                task_id,
                command_id,
                input: CommandInput::Text {
                    text: "Prove that this turn is live.".to_owned(),
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

pub(crate) async fn collect_through_model_request(socket: &mut Socket) -> Vec<TaskEvent> {
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
        panic!("expected text websocket message");
    };
    serde_json::from_str(&json).expect("deserialize server message")
}
