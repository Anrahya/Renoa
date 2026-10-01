//! The Discord surface over a real RCP coordinator, a scripted execution node,
//! and a fake Discord gateway and REST API.

use std::{
    sync::{Arc, Mutex},
    time::{Duration, SystemTime},
};

use futures_util::{SinkExt as _, StreamExt as _};
use renoa_control::{
    ClientMessage, Coordinator, DeviceCredentials, JSON_WS_VERSION, NodeId, PeerIdentity,
    ServerMessage,
};
use renoa_protocol::{
    ExecutionEvent, ExecutionEventId, ExecutionEventKind, ExecutionId, ExecutionTerminal,
    PrincipalId, SurfaceRef, TargetRef,
};
use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream, connect_async, tungstenite::Message};
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use crate::{
    api::DiscordApi,
    config::Rcp,
    service::{self, Surface},
    snowflake::Snowflake,
};

type Socket = WebSocketStream<MaybeTlsStream<tokio::net::TcpStream>>;

const ARCEE: &str = "11111111-1111-4111-8111-111111111111";
const DESK: &str = "22222222-2222-4222-8222-222222222222";

#[tokio::test]
async fn the_operators_mention_reaches_the_default_agent_as_the_owner_and_posts_one_reply() {
    let bodies = run(Scenario::Mention).await.posted;
    assert_eq!(bodies.len(), 1, "{bodies:?}");
    assert!(
        bodies[0].contains(&format!("agent:{ARCEE} answered Principal")),
        "{}",
        bodies[0]
    );
    assert!(bodies[0].contains(r"\nfrom yash (owner)"), "{}", bodies[0]);
}

#[tokio::test]
async fn a_bound_channel_reaches_its_agent_without_a_mention() {
    let bodies = run(Scenario::Bound).await.posted;
    assert_eq!(bodies.len(), 1, "{bodies:?}");
    assert!(
        bodies[0].contains(&format!("agent:{DESK} answered")),
        "{}",
        bodies[0]
    );
}

#[tokio::test]
async fn a_members_thread_message_reaches_its_parent_channels_agent_as_a_guest() {
    let observed = run(Scenario::Thread).await;
    let [answer] = observed.posted.as_slice() else {
        panic!("expected one answer: {:?}", observed.posted);
    };
    assert!(
        answer.starts_with("POST /channels/303/messages"),
        "{answer}"
    );
    assert!(
        answer.contains(&format!("agent:{DESK} answered")),
        "{answer}"
    );
    assert!(
        answer.contains(
            r#"Discord server 10\nchannel #desk (202)\nthread \"plan\" (303)\nfrom 99 (guest)"#
        ),
        "{answer}"
    );
    assert!(
        answer.contains(&format!("agent:{DESK} answered Guest")),
        "a member who is not the operator is submitted as a guest: {answer}"
    );
}

#[tokio::test]
async fn the_operators_new_is_answered_once_without_running_a_command() {
    let bodies = run(Scenario::New).await.posted;
    assert_eq!(bodies.len(), 1, "{bodies:?}");
    assert!(
        bodies[0].contains("Started a new conversation."),
        "{}",
        bodies[0]
    );
}

#[tokio::test]
async fn an_agent_without_an_online_node_gets_a_not_sent_reply() {
    let bodies = run(Scenario::Offline).await.posted;
    assert_eq!(bodies.len(), 1, "{bodies:?}");
    assert!(bodies[0].contains("offline"), "{}", bodies[0]);
}

#[tokio::test]
async fn a_running_command_shows_typing_and_its_tool_calls_before_the_answer() {
    let observed = run(Scenario::Tools).await;
    assert!(
        observed
            .requests
            .iter()
            .any(|request| request.starts_with("POST /channels/202/typing")),
        "{:?}",
        observed.requests
    );
    let [progress, answer] = observed.posted.as_slice() else {
        panic!(
            "expected a progress message and one answer: {:?}",
            observed.posted
        );
    };
    assert!(progress.contains("Working"), "{progress}");
    assert!(progress.contains("Checking the plugins."), "{progress}");
    assert!(progress.contains("plugin_search"), "{progress}");
    assert!(!progress.contains("answered"), "{progress}");
    assert!(
        answer.contains(&format!("agent:{ARCEE} answered")),
        "{answer}"
    );
    let position = |prefix: &str| {
        observed
            .requests
            .iter()
            .position(|request| request.starts_with(prefix))
    };
    let answered = observed
        .requests
        .iter()
        .position(|request| request.contains("answered"))
        .expect("the answer is posted");
    let deleted = position("DELETE /channels/202/messages/900")
        .expect("the progress message is deleted after the command finishes");
    assert!(answered < deleted, "{:?}", observed.requests);
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Scenario {
    Mention,
    Bound,
    /// An unmentioned message in a thread of the bound channel.
    Thread,
    Offline,
    /// The node reports a tool call, pauses, then answers.
    Tools,
    /// The operator's `/new`.
    New,
}

struct Observed {
    /// Message bodies the surface posted, in order.
    posted: Vec<String>,
    /// Every Discord REST request line and body, in order.
    requests: Vec<String>,
}

async fn run(scenario: Scenario) -> Observed {
    let root = tempfile::tempdir().expect("temp root");
    let data = root.path().join("data");
    let coordinator = CoordinatorFixture::start(&root).await;
    let executions = Arc::new(Mutex::new(0_usize));
    if scenario != Scenario::Offline {
        let node = coordinator.node().await;
        tokio::spawn(scripted_node(
            node,
            Arc::clone(&executions),
            scenario == Scenario::Tools,
        ));
    }
    let rcp = Rcp {
        endpoint: coordinator.url.clone(),
        credentials: coordinator.surface().await,
    };
    if matches!(scenario, Scenario::Bound | Scenario::Thread) {
        bind_desk(&data);
    }

    let requests = Arc::new(Mutex::new(Vec::<String>::new()));
    let gateway = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("gateway listener");
    let gateway_port = gateway.local_addr().expect("gateway port").port();
    let http = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("http listener");
    let http_addr = http.local_addr().expect("http port");
    let recorded = Arc::clone(&requests);
    tokio::spawn(async move { serve_http(http, gateway_port, recorded).await });
    tokio::spawn(async move { serve_gateway(gateway, scenario).await });

    let shutdown = CancellationToken::new();
    let task_shutdown = shutdown.clone();
    let task = tokio::spawn(async move {
        service::run(
            Surface {
                guild_id: Snowflake::parse("10").expect("guild"),
                operator_user_id: Snowflake::parse("20").expect("operator"),
                token: "discord-token".to_owned(),
                data_directory: data,
                rcp,
            },
            Uuid::parse_str(ARCEE).expect("agent"),
            DiscordApi::with_origin("discord-token".to_owned(), format!("http://{http_addr}"))
                .expect("api"),
            task_shutdown,
        )
        .await
    });

    let settled = |requests: &[String]| {
        let answered = posted(requests).iter().any(|body| {
            ["answered", "offline", "Started a new conversation."]
                .iter()
                .any(|text| body.contains(text))
        });
        let cleared = scenario != Scenario::Tools
            || requests
                .iter()
                .any(|request| request.starts_with("DELETE /channels/202/messages/900"));
        answered && cleared
    };
    tokio::time::timeout(Duration::from_secs(30), async {
        while !settled(&requests.lock().expect("requests")) {
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    })
    .await
    .expect("timed out waiting for the Discord reply");
    // The gateway delivers the same message twice; neither a second command
    // nor a second reply may follow.
    tokio::time::sleep(Duration::from_millis(500)).await;
    let requests = requests.lock().expect("requests").clone();
    match scenario {
        Scenario::Offline => {}
        Scenario::New => assert_eq!(*executions.lock().expect("executions"), 0),
        _ => assert_eq!(*executions.lock().expect("executions"), 1),
    }
    shutdown.cancel();
    task.await.expect("service task").expect("service");
    coordinator.stop().await;
    Observed {
        posted: posted(&requests),
        requests,
    }
}

fn posted(requests: &[String]) -> Vec<String> {
    requests
        .iter()
        .filter(|request| {
            request.starts_with("POST /channels/") && request.contains("/messages HTTP")
        })
        .cloned()
        .collect()
}

fn bind_desk(data: &std::path::Path) {
    let store = crate::store::SurfaceStore::control(data).expect("control store");
    store
        .bind_identity(
            &Snowflake::parse("10").expect("guild"),
            &Snowflake::parse("20").expect("operator"),
            Uuid::parse_str(ARCEE).expect("agent"),
        )
        .expect("identity");
    store
        .bind_channel(
            &crate::DiscordBindingRequest {
                operation_id: Uuid::new_v4(),
                channel_id: "202".into(),
                agent_id: Uuid::parse_str(DESK).expect("desk"),
                expected_revision: 0,
            },
            "desk",
        )
        .expect("binding");
}

struct CoordinatorFixture {
    coordinator: Coordinator,
    url: String,
    node_id: NodeId,
    principal_id: PrincipalId,
    shutdown: CancellationToken,
    server: tokio::task::JoinHandle<()>,
}

impl CoordinatorFixture {
    async fn start(root: &tempfile::TempDir) -> Self {
        let coordinator =
            Coordinator::open(root.path().join("control.sqlite3")).expect("open coordinator");
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind coordinator");
        let url = format!("ws://{}/connect", listener.local_addr().expect("address"));
        let shutdown = CancellationToken::new();
        let (serving, stop) = (coordinator.clone(), shutdown.clone());
        let server = tokio::spawn(async move {
            serving
                .serve(listener, stop)
                .await
                .expect("serve coordinator");
        });
        Self {
            coordinator,
            url,
            node_id: NodeId::new(),
            principal_id: PrincipalId::new(),
            shutdown,
            server,
        }
    }

    async fn surface(&self) -> DeviceCredentials {
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
            .expect("surface enrollment");
        renoa_rcp_client::enroll(&self.url, token)
            .await
            .expect("enroll surface")
    }

    /// An owned node that advertises both agents.
    async fn node(&self) -> Socket {
        let token = self
            .coordinator
            .create_node_enrollment(
                self.node_id,
                self.principal_id,
                SystemTime::now() + Duration::from_mins(1),
            )
            .await
            .expect("node enrollment");
        let credentials = renoa_rcp_client::enroll(&self.url, token)
            .await
            .expect("enroll node");
        let (mut socket, _) = connect_async(&self.url).await.expect("connect node");
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
        send(
            &mut socket,
            &ClientMessage::AdvertiseTargets {
                targets: [ARCEE, DESK]
                    .iter()
                    .map(|agent| TargetRef::new(format!("agent:{agent}")))
                    .collect(),
            },
        )
        .await;
        socket
    }

    async fn stop(self) {
        self.shutdown.cancel();
        self.server.await.expect("coordinator task");
    }
}

/// Answers every command with `<target> answered <author> from <context>`;
/// with `tools`, first
/// reports an intermediate message and a tool call, then pauses.
async fn scripted_node(mut node: Socket, executions: Arc<Mutex<usize>>, tools: bool) {
    loop {
        let ServerMessage::Execute { task_id, command } = receive(&mut node).await else {
            continue;
        };
        *executions.lock().expect("executions") += 1;
        let command_id = command.command_id;
        send(
            &mut node,
            &ClientMessage::AcknowledgeExecution {
                task_id,
                command_id,
            },
        )
        .await;
        let execution_id = ExecutionId::new();
        let mut batches = vec![vec![ExecutionEventKind::ExecutionStarted]];
        if tools {
            batches[0].extend([
                ExecutionEventKind::TurnStarted,
                ExecutionEventKind::AssistantMessage {
                    text: "Checking the plugins.".to_owned(),
                },
                ExecutionEventKind::ToolStarted {
                    call_id: "search".to_owned(),
                    name: "plugin_search".to_owned(),
                    arguments: serde_json::json!({}),
                },
                ExecutionEventKind::ToolFinished {
                    call_id: "search".to_owned(),
                    output: "[]".to_owned(),
                    is_error: false,
                },
            ]);
            batches.push(Vec::new());
        }
        batches.last_mut().expect("batch").extend([
            ExecutionEventKind::AssistantMessage {
                text: format!(
                    "{} answered {:?} from {}",
                    command.target.as_str(),
                    command.input.author(),
                    command.input.context().unwrap_or("nowhere")
                ),
            },
            ExecutionEventKind::ExecutionTerminated {
                terminal: ExecutionTerminal::Completed,
            },
        ]);
        let mut sequence = 0;
        for (index, batch) in batches.into_iter().enumerate() {
            if index > 0 {
                // Long enough for the surface to post its progress message.
                tokio::time::sleep(Duration::from_millis(2500)).await;
            }
            let events = batch
                .into_iter()
                .map(|kind| {
                    let event = ExecutionEvent {
                        event_id: ExecutionEventId::new(),
                        execution_id,
                        sequence,
                        recorded_at_ms: 0,
                        kind,
                    };
                    sequence += 1;
                    event
                })
                .collect();
            send(
                &mut node,
                &ClientMessage::PublishExecutionEvents {
                    task_id,
                    command_id,
                    events,
                },
            )
            .await;
        }
    }
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

pub(crate) async fn serve_http(
    listener: tokio::net::TcpListener,
    gateway_port: u16,
    requests: Arc<Mutex<Vec<String>>>,
) {
    loop {
        let Ok((mut stream, _)) = listener.accept().await else {
            return;
        };
        let mut buffer = vec![0_u8; 8192];
        let Ok(read) = stream.read(&mut buffer).await else {
            continue;
        };
        let request = String::from_utf8_lossy(&buffer[..read]);
        let body = if request.contains(" /channels/") {
            requests.lock().expect("requests").push(request.to_string());
            r#"{"id":"900"}"#.to_owned()
        } else {
            format!(r#"{{"url":"ws://127.0.0.1:{gateway_port}"}}"#)
        };
        let response = format!(
            "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
            body.len()
        );
        let _ = stream.write_all(response.as_bytes()).await;
    }
}

async fn serve_gateway(listener: tokio::net::TcpListener, scenario: Scenario) {
    let Ok((stream, _)) = listener.accept().await else {
        return;
    };
    let mut socket = tokio_tungstenite::accept_async(stream)
        .await
        .expect("accept gateway");
    socket
        .send(Message::Text(
            r#"{"op":10,"d":{"heartbeat_interval":60000}}"#.into(),
        ))
        .await
        .expect("hello");
    let _ = socket.next().await;
    socket
        .send(Message::Text(
            r#"{"op":0,"s":1,"t":"READY","d":{"session_id":"sess","resume_gateway_url":"ws://127.0.0.1","user":{"id":"50"}}}"#.into(),
        ))
        .await
        .expect("ready");
    if scenario == Scenario::Thread {
        socket
            .send(Message::Text(
                r#"{"op":0,"s":2,"t":"GUILD_CREATE","d":{"id":"10","channels":[{"id":"202","type":0,"name":"desk"}],"threads":[{"id":"303","type":11,"name":"plan","parent_id":"202"}]}}"#.into(),
            ))
            .await
            .expect("guild");
    }
    let message = match scenario {
        Scenario::Bound => {
            r#"{"op":0,"s":3,"t":"MESSAGE_CREATE","d":{"id":"101","channel_id":"202","guild_id":"10","content":"Do the real task.","author":{"id":"99"},"mentions":[]}}"#
        }
        Scenario::Thread => {
            r#"{"op":0,"s":3,"t":"MESSAGE_CREATE","d":{"id":"101","channel_id":"303","guild_id":"10","content":"Plan it here.","author":{"id":"99"},"mentions":[]}}"#
        }
        Scenario::New => {
            r#"{"op":0,"s":3,"t":"MESSAGE_CREATE","d":{"id":"101","channel_id":"202","guild_id":"10","content":"<@50> /new","author":{"id":"20","username":"yash"},"mentions":[{"id":"50"}]}}"#
        }
        Scenario::Mention | Scenario::Offline | Scenario::Tools => {
            r#"{"op":0,"s":3,"t":"MESSAGE_CREATE","d":{"id":"101","channel_id":"202","guild_id":"10","content":"<@50> Do the real task.","author":{"id":"20","username":"yash"},"mentions":[{"id":"50"}]}}"#
        }
    };
    socket
        .send(Message::Text(message.into()))
        .await
        .expect("message");
    tokio::time::sleep(Duration::from_millis(200)).await;
    socket
        .send(Message::Text(message.into()))
        .await
        .expect("duplicate message");
    let _ = socket.next().await;
}
