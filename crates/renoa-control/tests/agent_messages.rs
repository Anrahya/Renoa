//! Agents messaging agents through the coordinator: who may send, what a
//! refusal leaves behind, how the answer returns, and the chain backstop.

use std::time::{Duration, SystemTime};

use futures_util::{SinkExt, StreamExt};
use renoa_control::{
    ClientMessage, Coordinator, DeviceCredentials, ErrorCode, JSON_WS_VERSION, NodeId,
    PeerIdentity, ServerMessage, TargetSummary, TaskEvent, TaskEventKind, TaskId, TaskSpec,
};
use renoa_protocol::{
    AgentMessageKind, CommandEnvelope, CommandId, CommandInput, ExecutionEvent, ExecutionEventId,
    ExecutionEventKind, ExecutionId, ExecutionTerminal, PrincipalId, SurfaceRef, TargetCard,
    TargetRef,
};
use rusqlite::Connection;
use tempfile::TempDir;
use tokio::{
    net::TcpListener,
    time::{sleep, timeout},
};
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream, connect_async, tungstenite::Message};
use tokio_util::sync::CancellationToken;

type Socket = WebSocketStream<MaybeTlsStream<tokio::net::TcpStream>>;

const DESK: &str = "agent:desk";
const RESEARCH: &str = "agent:research";

struct System {
    files: TempDir,
    coordinator: Coordinator,
    url: String,
    owner: PrincipalId,
    shutdown: CancellationToken,
    server: Option<tokio::task::JoinHandle<()>>,
}

impl System {
    async fn start() -> Self {
        let files = TempDir::new().expect("temporary directory");
        let coordinator =
            Coordinator::open(files.path().join("control.sqlite")).expect("open coordinator");
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
        let address = listener.local_addr().expect("address");
        let shutdown = CancellationToken::new();
        let server = tokio::spawn({
            let coordinator = coordinator.clone();
            let shutdown = shutdown.clone();
            async move {
                coordinator
                    .serve(listener, shutdown)
                    .await
                    .expect("serve coordinator");
            }
        });
        Self {
            files,
            coordinator,
            url: format!("ws://{address}/connect"),
            owner: PrincipalId::new(),
            shutdown,
            server: Some(server),
        }
    }

    async fn stop(mut self) {
        self.shutdown.cancel();
        self.server.take().expect("server").await.expect("stopped");
    }

    fn database(&self) -> Connection {
        Connection::open(self.files.path().join("control.sqlite")).expect("open control database")
    }

    fn count(&self, sql: &str) -> i64 {
        self.database()
            .query_row(sql, [], |row| row.get(0))
            .expect("count rows")
    }

    /// Everything a refusal must leave untouched.
    fn footprint(&self) -> [i64; 4] {
        [
            self.count("SELECT COUNT(*) FROM tasks"),
            self.count("SELECT COUNT(*) FROM commands"),
            self.count("SELECT COUNT(*) FROM task_events"),
            self.count("SELECT COUNT(*) FROM pending_executions"),
        ]
    }

    async fn enroll(&self, peer: PeerIdentity) -> DeviceCredentials {
        let token = self
            .coordinator
            .create_enrollment(peer, SystemTime::now() + Duration::from_mins(1))
            .await
            .expect("create enrollment");
        self.claim(token).await
    }

    async fn enroll_node(&self, node_id: NodeId, owner: PrincipalId) -> DeviceCredentials {
        let token = self
            .coordinator
            .create_node_enrollment(node_id, owner, SystemTime::now() + Duration::from_mins(1))
            .await
            .expect("create node enrollment");
        self.claim(token).await
    }

    async fn claim(&self, token: renoa_control::EnrollmentToken) -> DeviceCredentials {
        let (mut socket, _) = connect_async(&self.url).await.expect("connect");
        send(
            &mut socket,
            &ClientMessage::Enroll {
                version: JSON_WS_VERSION,
                token,
            },
        )
        .await;
        let ServerMessage::Enrolled { credentials, .. } = receive(&mut socket).await else {
            panic!("expected enrollment");
        };
        credentials
    }

    async fn connect(&self, credentials: &DeviceCredentials) -> Socket {
        let (mut socket, _) = connect_async(&self.url).await.expect("connect");
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

    async fn surface(&self, principal: PrincipalId) -> Socket {
        let credentials = self
            .enroll(PeerIdentity::Surface {
                principal_id: principal,
                surface: SurfaceRef::new("test"),
            })
            .await;
        self.connect(&credentials).await
    }

    async fn create_task(&self, task_id: TaskId, node_id: NodeId, target: &str) {
        self.coordinator
            .create_task(TaskSpec {
                task_id,
                principal_id: self.owner,
                node_id,
                target: TargetRef::new(target),
            })
            .await
            .expect("create task");
    }
}

/// Two agents on two nodes of one owner, the first with a chat task and a
/// command the surface submitted and its node accepted.
struct World {
    system: System,
    desk_node: NodeId,
    research_node: NodeId,
    desk_credentials: DeviceCredentials,
    desk: Socket,
    research: Socket,
    surface: Socket,
    chat: TaskId,
    asked: CommandId,
}

impl World {
    async fn start() -> Self {
        let system = System::start().await;
        let desk_node = NodeId::new();
        let research_node = NodeId::new();
        let desk_credentials = system.enroll_node(desk_node, system.owner).await;
        let research_credentials = system.enroll_node(research_node, system.owner).await;
        let mut desk = system.connect(&desk_credentials).await;
        let mut research = system.connect(&research_credentials).await;
        advertise(&mut desk, &[(DESK, "Desk")]).await;
        advertise(&mut research, &[(RESEARCH, "Research")]).await;
        let mut surface = system.surface(system.owner).await;
        wait_for_targets(&mut surface, 2).await;
        let chat = TaskId::new();
        system.create_task(chat, desk_node, DESK).await;
        let asked = CommandId::new();
        submit(&mut surface, chat, asked, "Find out the answer.").await;
        let ServerMessage::Execute { .. } = receive(&mut desk).await else {
            panic!("the desk node should receive the command");
        };
        acknowledge(&mut desk, chat, asked).await;
        Self {
            system,
            desk_node,
            research_node,
            desk_credentials,
            desk,
            research,
            surface,
            chat,
            asked,
        }
    }

    /// The desk agent, executing `from`, messages Research.
    async fn message_research(
        &mut self,
        from: CommandId,
        id: CommandId,
        text: &str,
    ) -> ServerMessage {
        send_message(&mut self.desk, from, id, self.research_node, RESEARCH, text).await
    }

    /// Research receives the command, accepts it and publishes `kinds` and a
    /// terminal event for it.
    async fn research_finishes(
        &mut self,
        id: CommandId,
        kinds: Vec<ExecutionEventKind>,
        terminal: ExecutionTerminal,
    ) -> (TaskId, Vec<ExecutionEvent>) {
        let ServerMessage::Execute { task_id, command } = receive(&mut self.research).await else {
            panic!("Research should receive the message");
        };
        assert_eq!(command.command_id, id);
        acknowledge(&mut self.research, task_id, id).await;
        let events = transcript(kinds, terminal);
        publish(&mut self.research, task_id, id, events.clone()).await;
        (task_id, events)
    }
}

#[tokio::test]
async fn a_message_reaches_a_thread_of_the_agent_with_the_senders_identity() {
    let mut world = World::start().await;
    let first = CommandId::new();
    assert_eq!(
        world
            .message_research(world.asked, first, "What is the answer?")
            .await,
        ServerMessage::CommandAccepted {
            request_id: 7,
            command_id: first,
        }
    );

    let ServerMessage::Execute { task_id, command } = receive(&mut world.research).await else {
        panic!("Research should receive the message");
    };
    assert_eq!(command.command_id, first);
    assert_eq!(command.principal_id, world.system.owner);
    assert_eq!(command.surface, SurfaceRef::new("agent"));
    assert_eq!(command.target, TargetRef::new(RESEARCH));
    assert_eq!(
        command.input,
        CommandInput::from_text("What is the answer?")
    );
    let origin = command.origin.expect("the message names its sender");
    assert_eq!(origin.from_command, world.asked);
    assert_eq!(origin.from, TargetRef::new(DESK));
    assert_eq!(origin.name, "Desk");
    assert_eq!(origin.depth, 1);
    assert_eq!(origin.kind, AgentMessageKind::Request);

    // A surface of the owner finds the thread and reads the same record.
    send(
        &mut world.surface,
        &ClientMessage::ListTasks { request_id: 3 },
    )
    .await;
    let ServerMessage::TaskList { tasks, .. } = receive(&mut world.surface).await else {
        panic!("expected a task list");
    };
    assert!(
        tasks
            .iter()
            .any(|task| task.task_id == task_id && task.target == TargetRef::new(RESEARCH))
    );
    let replayed = attach_all(&mut world.surface, task_id).await;
    assert!(matches!(
        &replayed[..],
        [TaskEvent { kind: TaskEventKind::CommandSubmitted { command }, .. }]
            if command.command_id == first && command.origin.is_some()
    ));

    // A later message from the same agent reaches the same thread.
    let second = CommandId::new();
    let from_second_command = world.asked;
    world
        .message_research(from_second_command, second, "And one more thing?")
        .await;
    let ServerMessage::Execute {
        task_id: again,
        command,
    } = receive(&mut world.research).await
    else {
        panic!("Research should receive the second message");
    };
    assert_eq!((again, command.command_id), (task_id, second));
    world.system.stop().await;
}

#[tokio::test]
async fn an_exact_retry_admits_once_and_a_changed_retry_conflicts() {
    let mut world = World::start().await;
    let id = CommandId::new();
    let accepted = ServerMessage::CommandAccepted {
        request_id: 7,
        command_id: id,
    };
    assert_eq!(
        world.message_research(world.asked, id, "Ask once.").await,
        accepted
    );
    assert_eq!(
        world.message_research(world.asked, id, "Ask once.").await,
        accepted
    );
    assert_eq!(
        world
            .system
            .count("SELECT COUNT(*) FROM commands WHERE command_json LIKE '%\"origin\"%'"),
        1
    );
    // The still-pending delivery is offered again, as for any command.
    for _ in 0..2 {
        let ServerMessage::Execute { command, .. } = receive(&mut world.research).await else {
            panic!("expected a delivery");
        };
        assert_eq!(command.command_id, id);
    }

    let ServerMessage::Error { code, .. } =
        world.message_research(world.asked, id, "Ask twice.").await
    else {
        panic!("a changed retry is refused");
    };
    assert_eq!(code, ErrorCode::Conflict);

    // Today's availability does not rewrite what was admitted.
    disconnect(&mut world.research).await;
    wait_for_targets(&mut world.surface, 1).await;
    assert_eq!(
        world.message_research(world.asked, id, "Ask once.").await,
        accepted
    );

    // Nor does a rename since, or an advertisement that has not arrived yet
    // after a reconnect.
    advertise(&mut world.desk, &[(DESK, "Renamed desk")]).await;
    assert_eq!(
        world.message_research(world.asked, id, "Ask once.").await,
        accepted
    );
    advertise(&mut world.desk, &[]).await;
    assert_eq!(
        world.message_research(world.asked, id, "Ask once.").await,
        accepted
    );
    let before = world.system.footprint();
    assert_eq!(
        world.message_research(world.asked, id, "Ask once.").await,
        accepted
    );
    assert_eq!(world.system.footprint(), before);
    world.system.stop().await;
}

#[tokio::test]
async fn a_refused_message_leaves_no_task_command_or_record() {
    let mut world = World::start().await;
    let offline_node = NodeId::new();
    world
        .system
        .enroll_node(offline_node, world.system.owner)
        .await;
    let stranger_node = NodeId::new();
    let stranger_credentials = world
        .system
        .enroll_node(stranger_node, PrincipalId::new())
        .await;
    let mut stranger = world.system.connect(&stranger_credentials).await;
    advertise(&mut stranger, &[(RESEARCH, "Not yours")]).await;
    // The stranger's node is online; wait until it is listed for its owner.
    sleep(Duration::from_millis(200)).await;
    let before = world.system.footprint();

    let (asked, research, desk) = (world.asked, world.research_node, world.desk_node);
    let long = "x".repeat(64 * 1024 + 1);
    let cases: [(&str, CommandId, NodeId, &str, &str, ErrorCode); 7] = [
        (
            "unknown command",
            CommandId::new(),
            research,
            RESEARCH,
            "Hi.",
            ErrorCode::NotFound,
        ),
        (
            "offline agent",
            asked,
            offline_node,
            RESEARCH,
            "Hi.",
            ErrorCode::NodeOffline,
        ),
        (
            "unadvertised agent",
            asked,
            research,
            "agent:ghost",
            "Hi.",
            ErrorCode::NotFound,
        ),
        (
            "another owner's agent",
            asked,
            stranger_node,
            RESEARCH,
            "Hi.",
            ErrorCode::NotFound,
        ),
        (
            "itself",
            asked,
            desk,
            DESK,
            "Hi.",
            ErrorCode::InvalidMessage,
        ),
        (
            "empty text",
            asked,
            research,
            RESEARCH,
            "  ",
            ErrorCode::InvalidMessage,
        ),
        (
            "overlong text",
            asked,
            research,
            RESEARCH,
            &long,
            ErrorCode::InvalidMessage,
        ),
    ];
    for (label, from, node, target, text, expected) in cases {
        let reply = send_message(&mut world.desk, from, CommandId::new(), node, target, text).await;
        assert_refused(label, reply, expected);
        assert_eq!(world.system.footprint(), before, "{label} left residue");
    }

    // Research's node may not speak for a command the desk node executes.
    let reply = send_message(
        &mut world.research,
        asked,
        CommandId::new(),
        desk,
        DESK,
        "Hi.",
    )
    .await;
    assert_refused("another node's command", reply, ErrorCode::NotFound);
    assert_eq!(
        world.system.footprint(),
        before,
        "a foreign command left residue"
    );
    world.system.stop().await;
}

fn assert_refused(label: &str, reply: ServerMessage, expected: ErrorCode) {
    let ServerMessage::Error {
        request_id,
        code,
        message,
    } = reply
    else {
        panic!("{label}: the message should be refused");
    };
    assert_eq!(
        (request_id, code),
        (Some(7), expected),
        "{label}: {message}"
    );
    assert!(!message.is_empty(), "{label}");
}

#[tokio::test]
async fn a_node_without_an_owner_neither_lists_nor_messages() {
    let system = System::start().await;
    let node_id = NodeId::new();
    let credentials = system.enroll(PeerIdentity::Node { node_id }).await;
    let mut node = system.connect(&credentials).await;

    send(&mut node, &ClientMessage::ListTargets { request_id: 4 }).await;
    assert!(matches!(
        receive(&mut node).await,
        ServerMessage::Error {
            request_id: Some(4),
            code: ErrorCode::InvalidRole,
            ..
        }
    ));
    let reply = send_message(
        &mut node,
        CommandId::new(),
        CommandId::new(),
        node_id,
        DESK,
        "Hi.",
    )
    .await;
    assert!(matches!(
        reply,
        ServerMessage::Error {
            request_id: Some(7),
            code: ErrorCode::InvalidRole,
            ..
        }
    ));
    system.stop().await;
}

#[tokio::test]
async fn a_node_lists_its_owners_agents_by_name_and_nobody_elses() {
    let mut world = World::start().await;
    send(
        &mut world.desk,
        &ClientMessage::ListTargets { request_id: 9 },
    )
    .await;
    let ServerMessage::TargetList {
        request_id: 9,
        targets,
    } = receive(&mut world.desk).await
    else {
        panic!("expected a target list");
    };
    let mut named = targets
        .iter()
        .map(|summary| (summary.target.as_str().to_owned(), summary.name.clone()))
        .collect::<Vec<_>>();
    named.sort();
    assert_eq!(
        named,
        [
            (DESK.to_owned(), "Desk".to_owned()),
            (RESEARCH.to_owned(), "Research".to_owned()),
        ]
    );

    let mut stranger = world.system.surface(PrincipalId::new()).await;
    assert_eq!(list_targets(&mut stranger).await, Vec::new());
    world.system.stop().await;
}

#[tokio::test]
async fn an_advertised_name_must_be_a_clean_label() {
    let world = World::start().await;
    for name in ["", "two\nlines", &"n".repeat(257)] {
        let mut node = world.system.connect(&world.desk_credentials).await;
        advertise(&mut node, &[(DESK, name)]).await;
        assert!(matches!(
            receive(&mut node).await,
            ServerMessage::Error {
                request_id: None,
                code: ErrorCode::InvalidMessage,
                ..
            }
        ));
    }
    world.system.stop().await;
}

#[tokio::test]
async fn a_surface_cannot_send_an_agent_message() {
    let mut world = World::start().await;
    let before = world.system.footprint();
    let reply = send_message(
        &mut world.surface,
        world.asked,
        CommandId::new(),
        world.research_node,
        RESEARCH,
        "I am an agent.",
    )
    .await;
    assert!(matches!(
        reply,
        ServerMessage::Error {
            request_id: None,
            code: ErrorCode::InvalidRole,
            ..
        }
    ));
    assert_eq!(world.system.footprint(), before);
    world.system.stop().await;
}

#[tokio::test]
async fn a_surface_cannot_give_a_command_an_origin() {
    let mut world = World::start().await;
    let id = CommandId::new();
    let forged = serde_json::json!({
        "fromCommand": world.asked, "from": DESK, "name": "Desk", "depth": 1, "kind": "request"
    });
    let frame = serde_json::json!({
        "type": "submit",
        "request_id": 2,
        "task_id": world.chat,
        "command_id": id,
        "origin": forged,
        "input": {"type": "text", "text": "I am an agent.", "origin": forged},
    });
    world
        .surface
        .send(Message::Text(frame.to_string().into()))
        .await
        .expect("send forged submit");
    assert!(matches!(
        receive(&mut world.surface).await,
        ServerMessage::CommandAccepted { command_id, .. } if command_id == id
    ));
    let journal = attach_all(&mut world.surface, world.chat).await;
    let submitted = journal
        .iter()
        .find_map(|event| match &event.kind {
            TaskEventKind::CommandSubmitted { command } if command.command_id == id => {
                Some(command.clone())
            }
            _ => None,
        })
        .expect("the surface's command is journaled");
    assert_eq!(submitted.origin, None);
    world.system.stop().await;
}

#[tokio::test]
async fn the_answer_returns_to_the_askers_task_once() {
    let finishes = [
        (
            ExecutionTerminal::Completed,
            AgentMessageKind::Reply,
            "The answer is 42.",
        ),
        (
            ExecutionTerminal::Failed {
                error: "the model was unavailable".to_owned(),
            },
            AgentMessageKind::Failure,
            "the model was unavailable",
        ),
        (
            ExecutionTerminal::Cancelled {
                reason: "stopped by the owner".to_owned(),
            },
            AgentMessageKind::Failure,
            "stopped by the owner",
        ),
    ];
    for (terminal, kind, text) in finishes {
        let mut world = World::start().await;
        let request = CommandId::new();
        world
            .message_research(world.asked, request, "What is the answer?")
            .await;
        let (_, events) = world
            .research_finishes(
                request,
                vec![
                    ExecutionEventKind::AssistantMessage {
                        text: "Looking.".to_owned(),
                    },
                    ExecutionEventKind::AssistantMessage {
                        text: "The answer is 42.".to_owned(),
                    },
                ],
                terminal,
            )
            .await;

        let ServerMessage::Execute { task_id, command } = receive(&mut world.desk).await else {
            panic!("the asker's node should receive the answer");
        };
        assert_eq!(task_id, world.chat);
        assert_eq!(command.principal_id, world.system.owner);
        assert_eq!(command.surface, SurfaceRef::new("agent"));
        assert_eq!(command.target, TargetRef::new(DESK));
        assert_eq!(command.input.text(), text);
        let origin = command.origin.clone().expect("the answer names its sender");
        assert_eq!(origin.from_command, request);
        assert_eq!(origin.from, TargetRef::new(RESEARCH));
        assert_eq!(origin.name, "Research");
        assert_eq!(origin.depth, 2);
        assert_eq!(origin.kind, kind);

        // The asker's surface reads the answer as a new message in its task.
        let journal = attach_all(&mut world.surface, world.chat).await;
        assert!(matches!(
            journal.last().map(|event| &event.kind),
            Some(TaskEventKind::CommandSubmitted { command: last }) if last.command_id == command.command_id
        ));

        // The same batch again is acknowledged and answers nothing twice.
        let thread = world.chat_thread(request);
        publish(&mut world.research, thread, request, events).await;
        assert!(
            timeout(Duration::from_millis(300), world.desk.next())
                .await
                .is_err(),
            "a retried batch must not deliver a second answer"
        );
        assert_eq!(answers(&world.system), 1);

        // Running the answer answers nothing: only a message continues a chain.
        let reply = command.command_id;
        acknowledge(&mut world.desk, world.chat, reply).await;
        publish(
            &mut world.desk,
            world.chat,
            reply,
            transcript(
                vec![ExecutionEventKind::AssistantMessage {
                    text: "Noted.".to_owned(),
                }],
                ExecutionTerminal::Completed,
            ),
        )
        .await;
        assert_eq!(
            world
                .system
                .count("SELECT COUNT(*) FROM commands WHERE command_json LIKE '%\"origin\"%'"),
            2
        );
        world.system.stop().await;
    }
}

#[tokio::test]
async fn an_oversized_answer_is_cut_to_the_size_of_a_request() {
    let mut world = World::start().await;
    let request = CommandId::new();
    world
        .message_research(world.asked, request, "Write a lot.")
        .await;
    world
        .research_finishes(
            request,
            vec![ExecutionEventKind::AssistantMessage {
                text: "é".repeat(50_000),
            }],
            ExecutionTerminal::Completed,
        )
        .await;
    let ServerMessage::Execute { command, .. } = receive(&mut world.desk).await else {
        panic!("the asker's node should receive the answer");
    };
    let text = command.input.text();
    assert!(
        text.len() <= 64 * 1024 && text.len() > 60 * 1024,
        "{}",
        text.len()
    );
    world.system.stop().await;
}

#[tokio::test]
async fn an_answer_waits_for_an_asker_whose_node_is_offline() {
    let mut world = World::start().await;
    let request = CommandId::new();
    world
        .message_research(world.asked, request, "What is the answer?")
        .await;
    disconnect(&mut world.desk).await;
    wait_for_targets(&mut world.surface, 1).await;

    world
        .research_finishes(
            request,
            vec![ExecutionEventKind::AssistantMessage {
                text: "Done.".to_owned(),
            }],
            ExecutionTerminal::Completed,
        )
        .await;
    assert_eq!(
        answers(&world.system),
        1,
        "the answer is admitted while the asker is away"
    );

    let mut desk = world.system.connect(&world.desk_credentials).await;
    let ServerMessage::Execute { task_id, command } = receive(&mut desk).await else {
        panic!("the reconnecting node should receive the pending answer");
    };
    assert_eq!(task_id, world.chat);
    assert_eq!(command.input.text(), "Done.");
    world.system.stop().await;
}

#[tokio::test]
async fn an_answer_for_a_deleted_asker_task_is_dropped_and_the_ending_still_lands() {
    let mut world = World::start().await;
    let request = CommandId::new();
    world
        .message_research(world.asked, request, "What is the answer?")
        .await;
    publish(
        &mut world.desk,
        world.chat,
        world.asked,
        transcript(Vec::new(), ExecutionTerminal::Completed),
    )
    .await;
    send(
        &mut world.surface,
        &ClientMessage::DeleteTask {
            request_id: 5,
            task_id: world.chat,
        },
    )
    .await;
    assert!(matches!(
        receive(&mut world.surface).await,
        ServerMessage::TaskDeleted { .. }
    ));

    world
        .research_finishes(request, Vec::new(), ExecutionTerminal::Completed)
        .await;
    assert_eq!(answers(&world.system), 0);
    assert!(
        timeout(Duration::from_millis(300), world.desk.next())
            .await
            .is_err()
    );
    world.system.stop().await;
}

#[tokio::test]
async fn a_chain_is_stopped_at_the_backstop_and_the_last_answer_still_returns() {
    let system = System::start().await;
    let node_id = NodeId::new();
    let credentials = system.enroll_node(node_id, system.owner).await;
    let mut node = system.connect(&credentials).await;
    advertise(&mut node, &[(DESK, "Desk"), (RESEARCH, "Research")]).await;
    let mut surface = system.surface(system.owner).await;
    wait_for_targets(&mut surface, 2).await;
    let chat = TaskId::new();
    system.create_task(chat, node_id, DESK).await;
    let first = CommandId::new();
    submit(&mut surface, chat, first, "Start.").await;

    // Each agent, executing the message it was sent, messages the other.
    let mut previous = first;
    for step in 1..=100_u32 {
        let id = CommandId::new();
        let target = if step % 2 == 1 { RESEARCH } else { DESK };
        let reply = send_message(&mut node, previous, id, node_id, target, "Your turn.").await;
        assert_eq!(
            reply,
            ServerMessage::CommandAccepted {
                request_id: 7,
                command_id: id
            },
            "step {step}"
        );
        previous = id;
    }
    assert_eq!(
        system.count("SELECT MAX(json_extract(command_json, '$.origin.depth')) FROM commands"),
        100
    );

    let before = system.footprint();
    let ServerMessage::Error { code, message, .. } = send_message(
        &mut node,
        previous,
        CommandId::new(),
        node_id,
        DESK,
        "Once more.",
    )
    .await
    else {
        panic!("the 101st request is refused");
    };
    assert_eq!(code, ErrorCode::InvalidMessage);
    assert!(message.contains("too long"), "{message}");
    assert!(
        !message.contains("100"),
        "the limit is not shown: {message}"
    );
    assert_eq!(system.footprint(), before, "the refusal left residue");

    // The answer to the deepest request starts nothing, so it is not refused.
    let thread = TaskId::from_uuid(
        system
            .database()
            .query_row(
                "SELECT task_id FROM commands WHERE command_id = ?1",
                [previous.to_string()],
                |row| row.get::<_, String>(0),
            )
            .expect("thread of the deepest request")
            .parse()
            .expect("task id"),
    );
    acknowledge(&mut node, thread, previous).await;
    publish(
        &mut node,
        thread,
        previous,
        transcript(
            vec![ExecutionEventKind::AssistantMessage {
                text: "Enough.".to_owned(),
            }],
            ExecutionTerminal::Completed,
        ),
    )
    .await;
    assert_eq!(
        system.count("SELECT json_extract(command_json, '$.origin.depth') FROM commands WHERE command_json LIKE '%\"kind\":\"reply\"%'"),
        101
    );
    system.stop().await;
}

impl World {
    /// The thread task that holds `request`.
    fn chat_thread(&self, request: CommandId) -> TaskId {
        TaskId::from_uuid(
            self.system
                .database()
                .query_row(
                    "SELECT task_id FROM commands WHERE command_id = ?1",
                    [request.to_string()],
                    |row| row.get::<_, String>(0),
                )
                .expect("thread of the request")
                .parse()
                .expect("task id"),
        )
    }
}

/// The answers (replies and failures) the coordinator has admitted.
fn answers(system: &System) -> i64 {
    system.count(
        "SELECT COUNT(*) FROM commands
         WHERE json_extract(command_json, '$.origin.kind') IN ('reply', 'failure')",
    )
}

fn card(target: &str, name: &str) -> TargetCard {
    TargetCard {
        target: TargetRef::new(target),
        name: name.to_owned(),
    }
}

async fn advertise(node: &mut Socket, agents: &[(&str, &str)]) {
    send(
        node,
        &ClientMessage::AdvertiseTargets {
            targets: agents
                .iter()
                .map(|(target, name)| card(target, name))
                .collect(),
        },
    )
    .await;
}

async fn list_targets(surface: &mut Socket) -> Vec<TargetSummary> {
    send(surface, &ClientMessage::ListTargets { request_id: 5 }).await;
    let ServerMessage::TargetList { targets, .. } = receive(surface).await else {
        panic!("expected a target list");
    };
    targets
}

/// Advertisements arrive asynchronously, so poll until the expected count.
async fn wait_for_targets(surface: &mut Socket, expected: usize) {
    for _ in 0..100 {
        if list_targets(surface).await.len() == expected {
            return;
        }
        sleep(Duration::from_millis(20)).await;
    }
    panic!("expected {expected} advertised targets");
}

async fn submit(surface: &mut Socket, task_id: TaskId, command_id: CommandId, text: &str) {
    send(
        surface,
        &ClientMessage::Submit {
            request_id: 2,
            task_id,
            command_id,
            input: CommandInput::from_text(text),
        },
    )
    .await;
    assert_eq!(
        receive(surface).await,
        ServerMessage::CommandAccepted {
            request_id: 2,
            command_id
        }
    );
}

async fn acknowledge(node: &mut Socket, task_id: TaskId, command_id: CommandId) {
    send(
        node,
        &ClientMessage::AcknowledgeExecution {
            task_id,
            command_id,
        },
    )
    .await;
    assert_eq!(
        response(node).await,
        ServerMessage::ExecutionAcknowledged { command_id }
    );
}

async fn publish(
    node: &mut Socket,
    task_id: TaskId,
    command_id: CommandId,
    events: Vec<ExecutionEvent>,
) {
    let through_execution_sequence = events.last().expect("events").sequence;
    send(
        node,
        &ClientMessage::PublishExecutionEvents {
            task_id,
            command_id,
            events,
        },
    )
    .await;
    assert_eq!(
        response(node).await,
        ServerMessage::ExecutionEventsAccepted {
            command_id,
            through_execution_sequence
        }
    );
}

/// Started, turn started, `kinds`, then `terminal`.
fn transcript(kinds: Vec<ExecutionEventKind>, terminal: ExecutionTerminal) -> Vec<ExecutionEvent> {
    let execution_id = ExecutionId::new();
    let mut all = vec![
        ExecutionEventKind::ExecutionStarted,
        ExecutionEventKind::TurnStarted,
    ];
    all.extend(kinds);
    all.push(ExecutionEventKind::ExecutionTerminated { terminal });
    all.into_iter()
        .enumerate()
        .map(|(sequence, kind)| ExecutionEvent {
            event_id: ExecutionEventId::new(),
            execution_id,
            sequence: u64::try_from(sequence).expect("sequence"),
            recorded_at_ms: 1,
            kind,
        })
        .collect()
}

async fn send_message(
    node: &mut Socket,
    from_command: CommandId,
    message_id: CommandId,
    node_id: NodeId,
    target: &str,
    text: &str,
) -> ServerMessage {
    send(
        node,
        &ClientMessage::SendAgentMessage {
            request_id: 7,
            from_command,
            message_id,
            node_id,
            target: TargetRef::new(target),
            text: text.to_owned(),
        },
    )
    .await;
    response(node).await
}

/// Attaches from the start and returns the journal as it stands.
async fn attach_all(surface: &mut Socket, task_id: TaskId) -> Vec<TaskEvent> {
    send(
        surface,
        &ClientMessage::Attach {
            request_id: 11,
            task_id,
            after_sequence: None,
        },
    )
    .await;
    let ServerMessage::Attached {
        through_sequence, ..
    } = receive(surface).await
    else {
        panic!("expected an attachment");
    };
    let mut events = Vec::new();
    while through_sequence.is_some_and(|through| events.len() as u64 <= through) {
        let ServerMessage::TaskEvent { event } = receive(surface).await else {
            panic!("expected a replayed record");
        };
        events.push(event);
    }
    events
}

/// The next frame that answers a request or acknowledges an operation,
/// skipping deliveries.
async fn response(socket: &mut Socket) -> ServerMessage {
    loop {
        match receive(socket).await {
            ServerMessage::Execute { .. } => {}
            other => return other,
        }
    }
}

/// Ends the node's connection the way a machine going to sleep does.
async fn disconnect(socket: &mut Socket) {
    socket.close(None).await.expect("close the connection");
}

async fn send(socket: &mut Socket, message: &ClientMessage) {
    let value = serde_json::to_string(message).expect("serialize client message");
    socket
        .send(Message::Text(value.into()))
        .await
        .expect("send client message");
}

async fn receive(socket: &mut Socket) -> ServerMessage {
    loop {
        let message = timeout(Duration::from_secs(10), socket.next())
            .await
            .expect("timed out waiting for the coordinator")
            .expect("server message")
            .expect("valid websocket message");
        match message {
            Message::Text(value) => {
                return serde_json::from_str(&value).expect("deserialize server message");
            }
            Message::Ping(_) | Message::Pong(_) => {}
            other => panic!("expected text websocket message, got {other:?}"),
        }
    }
}

#[allow(
    dead_code,
    reason = "documents what a command envelope looks like to a node"
)]
fn _envelope(_: CommandEnvelope) {}
