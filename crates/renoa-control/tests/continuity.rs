use std::time::{Duration, SystemTime};

use futures_util::{SinkExt, StreamExt};
use renoa_control::{
    ClientMessage, Coordinator, DeviceCredentials, EnrollmentToken, ErrorCode, JSON_WS_VERSION,
    NodeId, PeerIdentity, ServerMessage, StopOutcome, TargetSummary, TaskEvent, TaskEventKind,
    TaskId, TaskSpec, TaskStatus, TaskSummary,
};
use renoa_protocol::{
    CommandEnvelope, CommandId, CommandInput, ExecutionEvent, ExecutionEventId, ExecutionEventKind,
    ExecutionId, ExecutionTerminal, PrincipalId, SurfaceRef, TargetCard, TargetRef,
};
use rusqlite::Connection;
use tempfile::TempDir;
use tokio::net::TcpListener;
use tokio::time::sleep;
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream, connect_async, tungstenite::Message};
use tokio_util::sync::CancellationToken;

type Socket = WebSocketStream<MaybeTlsStream<tokio::net::TcpStream>>;

mod discovery {
    use super::*;

    #[tokio::test]
    async fn a_surface_discovers_only_its_tasks_in_stable_order() {
        let system = TestSystem::start("workspace:primary").await;
        let second_task_id = TaskId::new();
        let second_target = TargetRef::new("workspace:secondary");
        system
            .coordinator
            .create_task(TaskSpec {
                task_id: second_task_id,
                principal_id: system.principal_id,
                node_id: system.node_id,
                target: second_target.clone(),
            })
            .await
            .expect("create second owned task");
        system
            .coordinator
            .create_task(TaskSpec {
                task_id: TaskId::new(),
                principal_id: PrincipalId::new(),
                node_id: system.node_id,
                target: TargetRef::new("workspace:foreign"),
            })
            .await
            .expect("create another principal's task");
        let mut expected = vec![
            TaskSummary {
                task_id: system.task_id,
                target: system.target.clone(),
                status: TaskStatus::Idle,
            },
            TaskSummary {
                task_id: second_task_id,
                target: second_target,
                status: TaskStatus::Idle,
            },
        ];
        expected.sort_by_key(|task| task.task_id.to_string());

        let mut surface = system.connect(&system.enroll_surface("mac").await).await;
        send(&mut surface, &ClientMessage::ListTasks { request_id: 7 }).await;
        assert_eq!(
            receive(&mut surface).await,
            ServerMessage::TaskList {
                request_id: 7,
                tasks: expected,
            }
        );

        system.stop().await;
    }
}

mod task_view {
    use super::*;

    #[tokio::test]
    async fn two_surfaces_observe_the_same_kernel_backed_turn() {
        let system = TestSystem::start("workspace:test").await;
        let mut node = system.connect(&system.enroll_node().await).await;
        let mut mac = system.connect(&system.enroll_surface("mac").await).await;
        let mut phone = system.connect(&system.enroll_surface("phone").await).await;
        attach_initial(&mut mac, system.task_id).await;
        attach_initial(&mut phone, system.task_id).await;

        let command_id = CommandId::new();
        submit(
            &mut mac,
            system.task_id,
            command_id,
            "Give me the final answer.",
        )
        .await;
        let command = execute_and_publish(&system, &mut node, command_id).await;
        assert_eq!(command.command_id, command_id);
        assert_eq!(command.principal_id, system.principal_id);
        assert_eq!(command.surface, SurfaceRef::new("mac"));
        assert_eq!(command.target, system.target);

        let mac_events = collect_through_terminal(&mut mac).await;
        let phone_events = collect_through_terminal(&mut phone).await;
        assert_eq!(mac_events, phone_events);
        assert!(matches!(
            mac_events.first().map(|event| &event.kind),
            Some(TaskEventKind::CommandSubmitted { command })
                if command.command_id == command_id
        ));
        assert!(matches!(
            mac_events.last().map(|event| &event.kind),
            Some(TaskEventKind::ExecutionEvent {
                command_id: cause,
                event,
            })
                if *cause == command_id && matches!(
                    &event.kind,
                    ExecutionEventKind::ExecutionTerminated {
                        terminal: ExecutionTerminal::Completed
                    }
                )
        ));
        assert!(
            mac_events
                .windows(2)
                .all(|events| events[1].sequence == events[0].sequence + 1)
        );

        system.stop().await;
    }
}

mod event_stream {
    use super::*;

    #[tokio::test]
    async fn a_surface_observes_execution_events_before_the_terminal_batch() {
        let system = TestSystem::start("workspace:stream").await;
        let mut node = system.connect(&system.enroll_node().await).await;
        let mut mac = system.connect(&system.enroll_surface("mac").await).await;
        attach_initial(&mut mac, system.task_id).await;
        let command_id = CommandId::new();

        submit(
            &mut mac,
            system.task_id,
            command_id,
            "Show progress before completion.",
        )
        .await;
        let (_, transcript) = execute(&system, &mut node, command_id).await;
        let first = transcript
            .events
            .first()
            .expect("execution start event")
            .clone();
        assert!(matches!(first.kind, ExecutionEventKind::ExecutionStarted));

        send(
            &mut node,
            &ClientMessage::PublishExecutionEvents {
                task_id: system.task_id,
                command_id,
                events: vec![first.clone()],
            },
        )
        .await;
        assert_eq!(
            receive(&mut node).await,
            ServerMessage::ExecutionEventsAccepted {
                command_id,
                through_execution_sequence: first.sequence,
            }
        );

        assert!(matches!(
            receive(&mut mac).await,
            ServerMessage::TaskEvent {
                event: TaskEvent {
                    kind: TaskEventKind::CommandSubmitted { command },
                    ..
                }
            } if command.command_id == command_id
        ));
        assert!(matches!(
            receive(&mut mac).await,
            ServerMessage::TaskEvent {
                event: TaskEvent {
                    kind: TaskEventKind::ExecutionEvent {
                        command_id: cause,
                        event,
                    },
                    ..
                }
            } if cause == command_id && event == first
        ));

        let remainder = transcript.events[1..].to_vec();
        let final_sequence = remainder.last().expect("terminal event").sequence;
        send(
            &mut node,
            &ClientMessage::PublishExecutionEvents {
                task_id: system.task_id,
                command_id,
                events: remainder,
            },
        )
        .await;
        assert_eq!(
            receive(&mut node).await,
            ServerMessage::ExecutionEventsAccepted {
                command_id,
                through_execution_sequence: final_sequence,
            }
        );
        assert!(matches!(
            collect_through_terminal(&mut mac)
                .await
                .last()
                .map(|event| &event.kind),
            Some(TaskEventKind::ExecutionEvent {
                event: ExecutionEvent {
                    kind: ExecutionEventKind::ExecutionTerminated { .. },
                    ..
                },
                ..
            })
        ));

        system.stop().await;
    }

    #[tokio::test]
    async fn a_lost_event_acknowledgement_replays_one_copy_after_restart() {
        let mut system = TestSystem::start("workspace:event-retry").await;
        let node_device = system.enroll_node().await;
        let mut node = system.connect(&node_device).await;
        let mut mac = system.connect(&system.enroll_surface("mac").await).await;
        attach_initial(&mut mac, system.task_id).await;
        let command_id = CommandId::new();

        submit(
            &mut mac,
            system.task_id,
            command_id,
            "Keep one copy after losing the acknowledgement.",
        )
        .await;
        let (_, transcript) = execute(&system, &mut node, command_id).await;
        let prefix = transcript.events[..2].to_vec();
        let through_execution_sequence = prefix.last().expect("event prefix").sequence;
        send(
            &mut node,
            &ClientMessage::PublishExecutionEvents {
                task_id: system.task_id,
                command_id,
                events: prefix.clone(),
            },
        )
        .await;

        assert!(matches!(
            receive(&mut mac).await,
            ServerMessage::TaskEvent {
                event: TaskEvent {
                    kind: TaskEventKind::CommandSubmitted { command },
                    ..
                }
            } if command.command_id == command_id
        ));
        for expected in &prefix {
            assert!(matches!(
                receive(&mut mac).await,
                ServerMessage::TaskEvent {
                    event: TaskEvent {
                        kind: TaskEventKind::ExecutionEvent {
                            command_id: cause,
                            event,
                        },
                        ..
                    }
                } if cause == command_id && event == *expected
            ));
        }
        node.close(None)
            .await
            .expect("lose the queued event acknowledgement");
        mac.close(None).await.expect("disconnect observing surface");
        system.restart_coordinator().await;

        let mut reconnected_node = system.connect(&node_device).await;
        send(
            &mut reconnected_node,
            &ClientMessage::PublishExecutionEvents {
                task_id: system.task_id,
                command_id,
                events: prefix.clone(),
            },
        )
        .await;
        assert_eq!(
            receive(&mut reconnected_node).await,
            ServerMessage::ExecutionEventsAccepted {
                command_id,
                through_execution_sequence,
            }
        );

        assert_replayed_prefix(&system, command_id, &prefix).await;

        system.stop().await;
    }

    #[tokio::test]
    async fn an_execution_event_gap_is_rejected_without_advancing_the_cursor() {
        let system = TestSystem::start("workspace:event-gap").await;
        let mut node = system.connect(&system.enroll_node().await).await;
        let mut mac = system.connect(&system.enroll_surface("mac").await).await;
        let command_id = CommandId::new();
        submit(
            &mut mac,
            system.task_id,
            command_id,
            "Keep the source order contiguous.",
        )
        .await;
        let (_, transcript) = execute(&system, &mut node, command_id).await;

        send(
            &mut node,
            &ClientMessage::PublishExecutionEvents {
                task_id: system.task_id,
                command_id,
                events: vec![transcript.events[0].clone()],
            },
        )
        .await;
        assert_eq!(
            receive(&mut node).await,
            ServerMessage::ExecutionEventsAccepted {
                command_id,
                through_execution_sequence: 0,
            }
        );

        send(
            &mut node,
            &ClientMessage::PublishExecutionEvents {
                task_id: system.task_id,
                command_id,
                events: vec![transcript.events[2].clone()],
            },
        )
        .await;
        assert!(matches!(
            receive(&mut node).await,
            ServerMessage::Error {
                code: ErrorCode::InvalidMessage,
                ..
            }
        ));

        let remainder = transcript.events[1..].to_vec();
        let final_sequence = remainder.last().expect("terminal event").sequence;
        send(
            &mut node,
            &ClientMessage::PublishExecutionEvents {
                task_id: system.task_id,
                command_id,
                events: remainder,
            },
        )
        .await;
        assert_eq!(
            receive(&mut node).await,
            ServerMessage::ExecutionEventsAccepted {
                command_id,
                through_execution_sequence: final_sequence,
            }
        );

        system.stop().await;
    }

    #[tokio::test]
    async fn a_changed_execution_event_retry_is_rejected_without_rewriting_history() {
        let system = TestSystem::start("workspace:event-conflict").await;
        let mut node = system.connect(&system.enroll_node().await).await;
        let mut mac = system.connect(&system.enroll_surface("mac").await).await;
        let command_id = CommandId::new();
        submit(
            &mut mac,
            system.task_id,
            command_id,
            "Do not rewrite accepted history.",
        )
        .await;
        let (_, transcript) = execute(&system, &mut node, command_id).await;
        let original = transcript.events[0].clone();

        send(
            &mut node,
            &ClientMessage::PublishExecutionEvents {
                task_id: system.task_id,
                command_id,
                events: vec![original.clone()],
            },
        )
        .await;
        assert_eq!(
            receive(&mut node).await,
            ServerMessage::ExecutionEventsAccepted {
                command_id,
                through_execution_sequence: 0,
            }
        );

        let mut changed = original.clone();
        changed.recorded_at_ms = changed.recorded_at_ms.saturating_add(1);
        send(
            &mut node,
            &ClientMessage::PublishExecutionEvents {
                task_id: system.task_id,
                command_id,
                events: vec![changed],
            },
        )
        .await;
        assert!(matches!(
            receive(&mut node).await,
            ServerMessage::Error {
                code: ErrorCode::Conflict,
                ..
            }
        ));

        send(
            &mut node,
            &ClientMessage::PublishExecutionEvents {
                task_id: system.task_id,
                command_id,
                events: vec![original],
            },
        )
        .await;
        assert_eq!(
            receive(&mut node).await,
            ServerMessage::ExecutionEventsAccepted {
                command_id,
                through_execution_sequence: 0,
            }
        );

        system.stop().await;
    }

    #[tokio::test]
    async fn a_terminal_execution_accepts_exact_retries_but_rejects_new_events() {
        let system = TestSystem::start("workspace:event-terminal").await;
        let mut node = system.connect(&system.enroll_node().await).await;
        let mut mac = system.connect(&system.enroll_surface("mac").await).await;
        let command_id = CommandId::new();
        submit(
            &mut mac,
            system.task_id,
            command_id,
            "Do not append after termination.",
        )
        .await;
        let (_, transcript) = execute(&system, &mut node, command_id).await;
        let terminal = transcript.events.last().expect("terminal event").clone();

        send(
            &mut node,
            &ClientMessage::PublishExecutionEvents {
                task_id: system.task_id,
                command_id,
                events: transcript.events.clone(),
            },
        )
        .await;
        assert_eq!(
            receive(&mut node).await,
            ServerMessage::ExecutionEventsAccepted {
                command_id,
                through_execution_sequence: terminal.sequence,
            }
        );

        send(
            &mut node,
            &ClientMessage::PublishExecutionEvents {
                task_id: system.task_id,
                command_id,
                events: transcript.events,
            },
        )
        .await;
        assert_eq!(
            receive(&mut node).await,
            ServerMessage::ExecutionEventsAccepted {
                command_id,
                through_execution_sequence: terminal.sequence,
            }
        );

        let extra = ExecutionEvent {
            event_id: ExecutionEventId::new(),
            execution_id: terminal.execution_id,
            sequence: terminal.sequence.saturating_add(1),
            recorded_at_ms: terminal.recorded_at_ms.saturating_add(1),
            kind: ExecutionEventKind::TurnStarted,
        };
        send(
            &mut node,
            &ClientMessage::PublishExecutionEvents {
                task_id: system.task_id,
                command_id,
                events: vec![extra],
            },
        )
        .await;
        assert!(matches!(
            receive(&mut node).await,
            ServerMessage::Error {
                code: ErrorCode::InvalidMessage,
                ..
            }
        ));

        system.stop().await;
    }
}

mod delivery {
    use super::*;

    #[tokio::test]
    async fn a_surface_replays_events_missed_while_disconnected() {
        let system = TestSystem::start("workspace:replay").await;
        let node_device = system.enroll_node().await;
        let mac_device = system.enroll_surface("mac").await;
        let phone_device = system.enroll_surface("phone").await;
        let mut node = system.connect(&node_device).await;
        let mut mac = system.connect(&mac_device).await;
        let mut phone = system.connect(&phone_device).await;
        attach_initial(&mut mac, system.task_id).await;
        attach_initial(&mut phone, system.task_id).await;

        let command_id = CommandId::new();
        submit(
            &mut mac,
            system.task_id,
            command_id,
            "Complete while my phone is gone.",
        )
        .await;
        let ServerMessage::TaskEvent { event: first_seen } = receive(&mut phone).await else {
            panic!("phone should observe the submitted command");
        };
        phone.close(None).await.expect("disconnect phone");

        execute_and_publish(&system, &mut node, command_id).await;
        let mac_events = collect_through_terminal(&mut mac).await;

        let mut reconnected_phone = system.connect(&phone_device).await;
        send(
            &mut reconnected_phone,
            &ClientMessage::Attach {
                request_id: 2,
                task_id: system.task_id,
                after_sequence: Some(first_seen.sequence),
                live: false,
            },
        )
        .await;
        assert_eq!(
            receive(&mut reconnected_phone).await,
            ServerMessage::Attached {
                request_id: 2,
                task_id: system.task_id,
                through_sequence: mac_events.last().map(|event| event.sequence),
            }
        );
        let replayed = collect_through_terminal(&mut reconnected_phone).await;
        assert_eq!(replayed, mac_events[1..]);

        system.stop().await;
    }

    #[tokio::test]
    async fn a_surface_cannot_attach_from_a_cursor_ahead_of_task_history() {
        let system = TestSystem::start("workspace:cursor").await;
        let mut surface = system.connect(&system.enroll_surface("mac").await).await;

        send(
            &mut surface,
            &ClientMessage::Attach {
                request_id: 2,
                task_id: system.task_id,
                after_sequence: Some(0),
                live: false,
            },
        )
        .await;
        assert!(matches!(
            receive(&mut surface).await,
            ServerMessage::Error {
                request_id: Some(2),
                code: ErrorCode::InvalidMessage,
                ..
            }
        ));

        system.stop().await;
    }

    #[tokio::test]
    async fn an_offline_node_rejects_work_without_queuing_it() {
        let system = TestSystem::start("workspace:offline").await;
        let mac_device = system.enroll_surface("mac").await;
        let mut mac = system.connect(&mac_device).await;
        attach_initial(&mut mac, system.task_id).await;

        send(
            &mut mac,
            &ClientMessage::Submit {
                request_id: 2,
                task_id: system.task_id,
                command_id: CommandId::new(),
                input: CommandInput::Text {
                    text: "Run whenever the node returns.".to_owned(),
                    context: None,
                    author: renoa_protocol::Author::Principal,
                },
            },
        )
        .await;
        assert!(matches!(
            receive(&mut mac).await,
            ServerMessage::Error {
                request_id: Some(2),
                code: ErrorCode::NodeOffline,
                ..
            }
        ));

        let mut reconnected_mac = system.connect(&mac_device).await;
        attach_initial(&mut reconnected_mac, system.task_id).await;
        system.stop().await;
    }

    #[tokio::test]
    async fn an_admitted_command_retry_is_accepted_while_its_node_is_offline() {
        let mut system = TestSystem::start("workspace:lost-ack").await;
        let node_device = system.enroll_node().await;
        let surface_device = system.enroll_surface("mac").await;
        let mut node = system.connect(&node_device).await;
        let mut surface = system.connect(&surface_device).await;
        let command_id = CommandId::new();
        let input = CommandInput::Text {
            text: "Accept this command once.".to_owned(),
            context: None,
            author: renoa_protocol::Author::Principal,
        };

        send(
            &mut surface,
            &ClientMessage::Submit {
                request_id: 2,
                task_id: system.task_id,
                command_id,
                input: input.clone(),
            },
        )
        .await;
        assert!(matches!(
            receive(&mut node).await,
            ServerMessage::Execute { command, .. } if command.command_id == command_id
        ));
        surface
            .close(None)
            .await
            .expect("disconnect before reading admission acknowledgement");
        node.close(None).await.expect("disconnect execution node");
        system.restart_coordinator().await;

        let mut reconnected = system.connect(&surface_device).await;
        send(
            &mut reconnected,
            &ClientMessage::Submit {
                request_id: 3,
                task_id: system.task_id,
                command_id,
                input,
            },
        )
        .await;
        assert_eq!(
            receive(&mut reconnected).await,
            ServerMessage::CommandAccepted {
                request_id: 3,
                command_id,
            }
        );

        send(
            &mut reconnected,
            &ClientMessage::Attach {
                request_id: 4,
                task_id: system.task_id,
                after_sequence: None,
                live: false,
            },
        )
        .await;
        assert_eq!(
            receive(&mut reconnected).await,
            ServerMessage::Attached {
                request_id: 4,
                task_id: system.task_id,
                through_sequence: Some(0),
            }
        );
        assert!(matches!(
            receive(&mut reconnected).await,
            ServerMessage::TaskEvent {
                event: TaskEvent {
                    sequence: 0,
                    kind: TaskEventKind::CommandSubmitted { command },
                    ..
                }
            } if command.command_id == command_id
        ));

        system.stop().await;
    }

    #[tokio::test]
    async fn an_unacknowledged_command_is_redelivered_after_node_reconnects() {
        let system = TestSystem::start("workspace:durable").await;
        let node_device = system.enroll_node().await;
        let mut node = system.connect(&node_device).await;
        let mut mac = system.connect(&system.enroll_surface("mac").await).await;
        let command_id = CommandId::new();

        submit(
            &mut mac,
            system.task_id,
            command_id,
            "Do not lose this command.",
        )
        .await;
        let first_delivery = receive(&mut node).await;
        assert!(matches!(
            &first_delivery,
            ServerMessage::Execute {
                task_id,
                command,
                ..
            } if *task_id == system.task_id && command.command_id == command_id
        ));
        node.close(None)
            .await
            .expect("disconnect before admission ack");

        let mut reconnected_node = system.connect(&node_device).await;
        assert_eq!(receive(&mut reconnected_node).await, first_delivery);
        send(
            &mut reconnected_node,
            &ClientMessage::AcknowledgeExecution {
                task_id: system.task_id,
                command_id,
            },
        )
        .await;
        assert_eq!(
            receive(&mut reconnected_node).await,
            ServerMessage::ExecutionAcknowledged { command_id }
        );
        reconnected_node
            .close(None)
            .await
            .expect("disconnect after admission ack");

        let mut final_node = system.connect(&node_device).await;
        let next_command_id = CommandId::new();
        submit(
            &mut mac,
            system.task_id,
            next_command_id,
            "Prove the old command is no longer pending.",
        )
        .await;
        assert!(matches!(
            receive(&mut final_node).await,
            ServerMessage::Execute { command, .. } if command.command_id == next_command_id
        ));

        system.stop().await;
    }

    #[tokio::test]
    async fn a_pending_command_survives_coordinator_restart() {
        let mut system = TestSystem::start("workspace:restart").await;
        let node_device = system.enroll_node().await;
        let mut node = system.connect(&node_device).await;
        let mut mac = system.connect(&system.enroll_surface("mac").await).await;
        let command_id = CommandId::new();

        submit(
            &mut mac,
            system.task_id,
            command_id,
            "Persist this before accepting it.",
        )
        .await;
        let first_delivery = receive(&mut node).await;
        assert!(matches!(
            &first_delivery,
            ServerMessage::Execute { command, .. } if command.command_id == command_id
        ));
        node.close(None).await.expect("disconnect node");
        mac.close(None).await.expect("disconnect surface");

        system.restart_coordinator().await;

        let mut reconnected_node = system.connect(&node_device).await;
        assert_eq!(receive(&mut reconnected_node).await, first_delivery);

        system.stop().await;
    }

    #[tokio::test]
    async fn replacing_a_node_cannot_strand_an_in_flight_admission() {
        let system = TestSystem::start("workspace:replacement").await;
        let node_device = system.enroll_node().await;
        let _old_node = system.connect(&node_device).await;
        let mut mac = system.connect(&system.enroll_surface("mac").await).await;
        let blocker = Connection::open(system.files.path().join("control.sqlite"))
            .expect("open coordinator database");
        blocker
            .execute_batch("BEGIN IMMEDIATE")
            .expect("hold coordinator write lock");
        let command_id = CommandId::new();

        send(
            &mut mac,
            &ClientMessage::Submit {
                request_id: 2,
                task_id: system.task_id,
                command_id,
                input: CommandInput::Text {
                    text: "Do not strand me between nodes.".to_owned(),
                    context: None,
                    author: renoa_protocol::Author::Principal,
                },
            },
        )
        .await;
        sleep(Duration::from_millis(50)).await;

        let (mut replacement, _) = connect_async(&system.url)
            .await
            .expect("connect replacement node");
        send(
            &mut replacement,
            &ClientMessage::Authenticate {
                version: JSON_WS_VERSION,
                credentials: node_device,
            },
        )
        .await;
        sleep(Duration::from_millis(50)).await;
        blocker
            .execute_batch("ROLLBACK")
            .expect("release coordinator write lock");

        assert_eq!(
            receive(&mut mac).await,
            ServerMessage::CommandAccepted {
                request_id: 2,
                command_id,
            }
        );
        assert_eq!(
            receive(&mut replacement).await,
            ServerMessage::Authenticated {
                version: JSON_WS_VERSION
            }
        );

        let sentinel_id = CommandId::new();
        send(
            &mut mac,
            &ClientMessage::Submit {
                request_id: 3,
                task_id: system.task_id,
                command_id: sentinel_id,
                input: CommandInput::Text {
                    text: "Expose any stranded predecessor.".to_owned(),
                    context: None,
                    author: renoa_protocol::Author::Principal,
                },
            },
        )
        .await;
        assert_eq!(
            receive(&mut mac).await,
            ServerMessage::CommandAccepted {
                request_id: 3,
                command_id: sentinel_id,
            }
        );
        assert!(matches!(
            receive(&mut replacement).await,
            ServerMessage::Execute { command, .. } if command.command_id == command_id
        ));

        system.stop().await;
    }

    #[tokio::test]
    async fn another_node_cannot_acknowledge_a_tasks_pending_command() {
        let system = TestSystem::start("workspace:bound").await;
        let owner_device = system.enroll_node().await;
        let mut owner = system.connect(&owner_device).await;
        let mut mac = system.connect(&system.enroll_surface("mac").await).await;
        let command_id = CommandId::new();

        submit(
            &mut mac,
            system.task_id,
            command_id,
            "Only my bound node may admit this.",
        )
        .await;
        let delivery = receive(&mut owner).await;
        owner.close(None).await.expect("disconnect task owner");

        let attacker_device = system
            .enroll(PeerIdentity::Node {
                node_id: NodeId::new(),
            })
            .await;
        let mut attacker = system.connect(&attacker_device).await;
        send(
            &mut attacker,
            &ClientMessage::AcknowledgeExecution {
                task_id: system.task_id,
                command_id,
            },
        )
        .await;
        assert!(matches!(
            receive(&mut attacker).await,
            ServerMessage::Error {
                code: ErrorCode::InvalidMessage,
                ..
            }
        ));

        let mut reconnected_owner = system.connect(&owner_device).await;
        assert_eq!(receive(&mut reconnected_owner).await, delivery);

        system.stop().await;
    }

    #[tokio::test]
    async fn retrying_an_acknowledged_command_is_an_execution_and_history_no_op() {
        let system = TestSystem::start("workspace:retry").await;
        let mut node = system.connect(&system.enroll_node().await).await;
        let mut mac = system.connect(&system.enroll_surface("mac").await).await;
        attach_initial(&mut mac, system.task_id).await;
        let command_id = CommandId::new();

        submit(&mut mac, system.task_id, command_id, "Retry me safely.").await;
        execute_and_publish(&system, &mut node, command_id).await;
        let original = collect_through_terminal(&mut mac).await;

        submit(&mut mac, system.task_id, command_id, "Retry me safely.").await;
        let next_command_id = CommandId::new();
        submit(
            &mut mac,
            system.task_id,
            next_command_id,
            "Follow the idempotent retry.",
        )
        .await;
        assert!(matches!(
            receive(&mut node).await,
            ServerMessage::Execute { command, .. } if command.command_id == next_command_id
        ));

        let mut observer = system
            .connect(&system.enroll_surface("observer").await)
            .await;
        send(
            &mut observer,
            &ClientMessage::Attach {
                request_id: 3,
                task_id: system.task_id,
                after_sequence: original.last().map(|event| event.sequence),
                live: false,
            },
        )
        .await;
        assert_eq!(
            receive(&mut observer).await,
            ServerMessage::Attached {
                request_id: 3,
                task_id: system.task_id,
                through_sequence: original
                    .last()
                    .map(|event| event.sequence.saturating_add(1)),
            }
        );
        assert!(matches!(
            receive(&mut observer).await,
            ServerMessage::TaskEvent {
                event: TaskEvent {
                    kind: TaskEventKind::CommandSubmitted { command },
                    ..
                }
            } if command.command_id == next_command_id
        ));

        system.stop().await;
    }

    #[tokio::test]
    async fn a_command_retry_must_repeat_its_context_and_author_exactly() {
        let system = TestSystem::start("workspace:context").await;
        let _node = system.connect(&system.enroll_node().await).await;
        let mut surface = system
            .connect(&system.enroll_surface("discord").await)
            .await;
        let command_id = CommandId::new();
        let submitted = |context: Option<&str>, author| ClientMessage::Submit {
            request_id: 2,
            task_id: system.task_id,
            command_id,
            input: CommandInput::Text {
                text: "Post it here.".to_owned(),
                context: context.map(str::to_owned),
                author,
            },
        };
        let desk = Some("channel #desk (202)");
        let guest = renoa_protocol::Author::Guest;
        let accepted = ServerMessage::CommandAccepted {
            request_id: 2,
            command_id,
        };
        for _ in 0..2 {
            send(&mut surface, &submitted(desk, guest)).await;
            assert_eq!(
                receive(&mut surface).await,
                accepted,
                "an exact retry converges"
            );
        }
        for changed in [
            (Some("channel #other (303)"), guest),
            (None, guest),
            (desk, renoa_protocol::Author::Principal),
        ] {
            send(&mut surface, &submitted(changed.0, changed.1)).await;
            assert!(
                matches!(
                    receive(&mut surface).await,
                    ServerMessage::Error {
                        request_id: Some(2),
                        code: ErrorCode::Conflict,
                        ..
                    }
                ),
                "{changed:?} changes the admitted command"
            );
        }
        system.stop().await;
    }
}

mod task_deletion {
    use super::*;

    async fn delete(surface: &mut Socket, task_id: TaskId) -> ServerMessage {
        send(
            surface,
            &ClientMessage::DeleteTask {
                request_id: 6,
                task_id,
            },
        )
        .await;
        receive(surface).await
    }

    fn rows_for(system: &TestSystem, task_id: TaskId) -> i64 {
        rusqlite::Connection::open(system.files.path().join("control.sqlite"))
            .expect("open coordinator store")
            .query_row(
                "SELECT (SELECT count(*) FROM tasks WHERE task_id = ?1)
                      + (SELECT count(*) FROM task_events WHERE task_id = ?1)
                      + (SELECT count(*) FROM commands WHERE task_id = ?1)
                      + (SELECT count(*) FROM execution_event_streams)
                      + (SELECT count(*) FROM pending_executions)
                      + (SELECT count(*) FROM stop_requests)",
                [task_id.to_string()],
                |row| row.get(0),
            )
            .expect("count task rows")
    }

    #[tokio::test]
    async fn an_owner_deletes_a_settled_task_and_a_retry_converges() {
        let system = TestSystem::start("workspace:delete").await;
        let mut node = system.connect(&system.enroll_node().await).await;
        let mut mac = system.connect(&system.enroll_surface("mac").await).await;
        let command_id = CommandId::new();
        submit(&mut mac, system.task_id, command_id, "Run once.").await;
        assert!(
            matches!(
                delete(&mut mac, system.task_id).await,
                ServerMessage::Error {
                    request_id: Some(6),
                    code: ErrorCode::Conflict,
                    ..
                }
            ),
            "a command pending delivery keeps its task"
        );
        let (_, transcript) = execute(&system, &mut node, command_id).await;
        assert!(
            matches!(
                delete(&mut mac, system.task_id).await,
                ServerMessage::Error {
                    request_id: Some(6),
                    code: ErrorCode::Conflict,
                    ..
                }
            ),
            "an acknowledged execution with no events yet keeps its task"
        );
        // A recorded stop is part of what the task holds, and goes with it.
        send(
            &mut mac,
            &ClientMessage::StopCommand {
                request_id: 7,
                task_id: system.task_id,
                command_id,
            },
        )
        .await;
        assert!(matches!(
            receive(&mut mac).await,
            ServerMessage::StopAccepted { request_id: 7, .. }
        ));
        assert!(matches!(
            receive(&mut node).await,
            ServerMessage::Stop { .. }
        ));
        let terminal = transcript.events.last().expect("terminal event").sequence;
        send(
            &mut node,
            &ClientMessage::PublishExecutionEvents {
                task_id: system.task_id,
                command_id,
                events: transcript.events,
            },
        )
        .await;
        assert_eq!(
            receive(&mut node).await,
            ServerMessage::ExecutionEventsAccepted {
                command_id,
                through_execution_sequence: terminal,
            }
        );
        assert!(rows_for(&system, system.task_id) > 0);

        let deleted = ServerMessage::TaskDeleted {
            request_id: 6,
            task_id: system.task_id,
        };
        assert_eq!(delete(&mut mac, system.task_id).await, deleted);
        assert_eq!(rows_for(&system, system.task_id), 0, "nothing is kept");
        assert_eq!(
            delete(&mut mac, system.task_id).await,
            deleted,
            "a retry converges"
        );
        send(
            &mut mac,
            &ClientMessage::Attach {
                request_id: 1,
                task_id: system.task_id,
                after_sequence: None,
                live: false,
            },
        )
        .await;
        assert!(matches!(
            receive(&mut mac).await,
            ServerMessage::Error {
                request_id: Some(1),
                code: ErrorCode::NotFound,
                ..
            }
        ));
        system.stop().await;
    }

    #[tokio::test]
    async fn another_principal_cannot_delete_a_task() {
        let system = TestSystem::start("workspace:delete").await;
        let stranger_device = system
            .enroll(PeerIdentity::Surface {
                principal_id: PrincipalId::new(),
                surface: SurfaceRef::new("stranger"),
            })
            .await;
        let mut stranger = system.connect(&stranger_device).await;
        assert!(matches!(
            delete(&mut stranger, system.task_id).await,
            ServerMessage::Error {
                request_id: Some(6),
                code: ErrorCode::NotFound,
                ..
            }
        ));
        assert_eq!(rows_for(&system, system.task_id), 1, "the task is intact");
        system.stop().await;
    }
}

mod task_opening {
    use super::*;

    const ALPHA: &str = "agent:alpha";
    const ARCEE: &str = "agent:arcee";

    #[tokio::test]
    async fn an_owner_opens_a_task_on_an_advertised_target_and_runs_it() {
        let system = TestSystem::start("workspace:static").await;
        let node_device = system.enroll_owned_node().await;
        let mut node = system.connect(&node_device).await;
        advertise(&mut node, &[ARCEE, ALPHA]).await;
        let mut surface = system
            .connect(&system.enroll_surface("discord").await)
            .await;

        assert_eq!(
            wait_for_targets(&mut surface, 2).await,
            vec![
                TargetSummary {
                    node_id: system.node_id,
                    target: TargetRef::new(ALPHA),
                    name: "Alpha".to_owned(),
                },
                TargetSummary {
                    node_id: system.node_id,
                    target: TargetRef::new(ARCEE),
                    name: "Arcee".to_owned(),
                },
            ]
        );
        let task_id = TaskId::new();
        assert_eq!(
            open_task(&mut surface, task_id, system.node_id, ALPHA).await,
            ServerMessage::TaskOpened {
                request_id: 3,
                task_id,
            }
        );
        assert!(list_tasks(&mut surface).await.contains(&TaskSummary {
            task_id,
            target: TargetRef::new(ALPHA),
            status: TaskStatus::Idle,
        }));

        attach_initial(&mut surface, task_id).await;
        let command_id = CommandId::new();
        submit(&mut surface, task_id, command_id, "Summarize today.").await;
        let ServerMessage::Execute {
            task_id: executed,
            command,
        } = receive(&mut node).await
        else {
            panic!("the node should execute the opened task");
        };
        assert_eq!(executed, task_id);
        assert_eq!(command.command_id, command_id);
        assert_eq!(command.target, TargetRef::new(ALPHA));
        system.stop().await;
    }

    #[tokio::test]
    async fn an_exact_retry_converges_after_the_node_leaves_and_a_changed_reuse_conflicts() {
        let system = TestSystem::start("workspace:static").await;
        let node_device = system.enroll_owned_node().await;
        let node = {
            let mut node = system.connect(&node_device).await;
            advertise(&mut node, &[ALPHA, ARCEE]).await;
            node
        };
        let mut surface = system
            .connect(&system.enroll_surface("discord").await)
            .await;
        wait_for_targets(&mut surface, 2).await;
        let task_id = TaskId::new();
        open_task(&mut surface, task_id, system.node_id, ALPHA).await;

        drop(node);
        wait_for_targets(&mut surface, 0).await;
        assert_eq!(
            open_task(&mut surface, task_id, system.node_id, ALPHA).await,
            ServerMessage::TaskOpened {
                request_id: 3,
                task_id,
            },
            "an exact retry observes its original opening while the node is offline"
        );
        assert!(matches!(
            open_task(&mut surface, task_id, system.node_id, ARCEE).await,
            ServerMessage::Error {
                request_id: Some(3),
                code: ErrorCode::Conflict,
                ..
            }
        ));
        system.stop().await;
    }

    #[tokio::test]
    async fn an_offline_or_unadvertised_target_is_rejected_without_a_task() {
        let system = TestSystem::start("workspace:static").await;
        let node_device = system.enroll_owned_node().await;
        let mut surface = system
            .connect(&system.enroll_surface("discord").await)
            .await;
        let before = list_tasks(&mut surface).await;

        assert!(matches!(
            open_task(&mut surface, TaskId::new(), system.node_id, ALPHA).await,
            ServerMessage::Error {
                request_id: Some(3),
                code: ErrorCode::NodeOffline,
                ..
            }
        ));
        let mut node = system.connect(&node_device).await;
        advertise(&mut node, &[ALPHA]).await;
        wait_for_targets(&mut surface, 1).await;
        assert!(matches!(
            open_task(&mut surface, TaskId::new(), system.node_id, ARCEE).await,
            ServerMessage::Error {
                request_id: Some(3),
                code: ErrorCode::NotFound,
                ..
            }
        ));

        assert_eq!(list_tasks(&mut surface).await, before);
        system.stop().await;
    }

    #[tokio::test]
    async fn another_principal_can_neither_see_nor_use_the_node() {
        let system = TestSystem::start("workspace:static").await;
        let mut node = system.connect(&system.enroll_owned_node().await).await;
        advertise(&mut node, &[ALPHA]).await;
        let mut owner = system
            .connect(&system.enroll_surface("discord").await)
            .await;
        wait_for_targets(&mut owner, 1).await;
        let stranger_device = system
            .enroll(PeerIdentity::Surface {
                principal_id: PrincipalId::new(),
                surface: SurfaceRef::new("stranger"),
            })
            .await;
        let mut stranger = system.connect(&stranger_device).await;

        assert_eq!(list_targets(&mut stranger).await, Vec::new());
        assert!(matches!(
            open_task(&mut stranger, TaskId::new(), system.node_id, ALPHA).await,
            ServerMessage::Error {
                request_id: Some(3),
                code: ErrorCode::NotFound,
                ..
            }
        ));
        system.stop().await;
    }

    #[tokio::test]
    async fn an_invalid_advertisement_is_rejected() {
        let system = TestSystem::start("workspace:static").await;
        let mut node = system.connect(&system.enroll_owned_node().await).await;

        advertise(&mut node, &[ALPHA, ALPHA]).await;

        assert!(matches!(
            receive(&mut node).await,
            ServerMessage::Error {
                request_id: None,
                code: ErrorCode::InvalidMessage,
                ..
            }
        ));
        system.stop().await;
    }

    async fn advertise(node: &mut Socket, targets: &[&str]) {
        send(
            node,
            &ClientMessage::AdvertiseTargets {
                targets: targets
                    .iter()
                    .map(|target| TargetCard {
                        target: TargetRef::new(*target),
                        name: card_name(target),
                    })
                    .collect(),
            },
        )
        .await;
    }

    /// `agent:alpha` is advertised as "Alpha".
    fn card_name(target: &str) -> String {
        let name = target.trim_start_matches("agent:");
        let mut letters = name.chars();
        letters.next().map_or_else(String::new, |first| {
            first.to_uppercase().chain(letters).collect()
        })
    }

    async fn open_task(
        surface: &mut Socket,
        task_id: TaskId,
        node_id: NodeId,
        target: &str,
    ) -> ServerMessage {
        send(
            surface,
            &ClientMessage::OpenTask {
                request_id: 3,
                task_id,
                node_id,
                target: TargetRef::new(target),
            },
        )
        .await;
        receive(surface).await
    }

    async fn list_tasks(surface: &mut Socket) -> Vec<TaskSummary> {
        send(surface, &ClientMessage::ListTasks { request_id: 4 }).await;
        let ServerMessage::TaskList {
            request_id: 4,
            tasks,
        } = receive(surface).await
        else {
            panic!("expected a task list");
        };
        tasks
    }

    async fn list_targets(surface: &mut Socket) -> Vec<TargetSummary> {
        send(surface, &ClientMessage::ListTargets { request_id: 5 }).await;
        let ServerMessage::TargetList {
            request_id: 5,
            targets,
        } = receive(surface).await
        else {
            panic!("expected a target list");
        };
        targets
    }

    /// Advertisements arrive asynchronously, so poll until the expected count.
    async fn wait_for_targets(surface: &mut Socket, expected: usize) -> Vec<TargetSummary> {
        for _ in 0..100 {
            let targets = list_targets(surface).await;
            if targets.len() == expected {
                return targets;
            }
            sleep(Duration::from_millis(20)).await;
        }
        panic!("expected {expected} advertised targets");
    }
}

mod stopping {
    use super::*;

    async fn stop(surface: &mut Socket, task_id: TaskId, command_id: CommandId) -> ServerMessage {
        send(
            surface,
            &ClientMessage::StopCommand {
                request_id: 11,
                task_id,
                command_id,
            },
        )
        .await;
        receive(surface).await
    }

    fn requested(command_id: CommandId) -> ServerMessage {
        ServerMessage::StopAccepted {
            request_id: 11,
            command_id,
            outcome: StopOutcome::Requested,
        }
    }

    fn stop_frame(system: &TestSystem, command_id: CommandId) -> ServerMessage {
        ServerMessage::Stop {
            task_id: system.task_id,
            command_id,
        }
    }

    /// The stop rows the coordinator holds, as `(command, delivered)`.
    fn stop_rows(system: &TestSystem) -> Vec<(String, bool)> {
        let connection = Connection::open(system.files.path().join("control.sqlite"))
            .expect("open coordinator store");
        let mut statement = connection
            .prepare("SELECT command_id, delivered FROM stop_requests ORDER BY rowid")
            .expect("prepare stop rows");
        statement
            .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
            .expect("query stop rows")
            .map(|row| row.expect("stop row"))
            .collect()
    }

    async fn acknowledge_stop(node: &mut Socket, system: &TestSystem, command_id: CommandId) {
        send(
            node,
            &ClientMessage::AcknowledgeStop {
                task_id: system.task_id,
                command_id,
            },
        )
        .await;
    }

    /// Answers once the node's earlier frames were handled: the coordinator
    /// handles one connection's frames in order, and acknowledging an
    /// execution is always answered. A frame the coordinator delivered to the
    /// node earlier would arrive first and fail the assertion.
    async fn barrier(node: &mut Socket, system: &TestSystem, command_id: CommandId) {
        send(
            node,
            &ClientMessage::AcknowledgeExecution {
                task_id: system.task_id,
                command_id,
            },
        )
        .await;
        assert_eq!(
            receive(node).await,
            ServerMessage::ExecutionAcknowledged { command_id }
        );
    }

    async fn publish(
        node: &mut Socket,
        system: &TestSystem,
        command_id: CommandId,
        events: Vec<ExecutionEvent>,
    ) {
        let through_execution_sequence = events.last().expect("events").sequence;
        send(
            node,
            &ClientMessage::PublishExecutionEvents {
                task_id: system.task_id,
                command_id,
                events,
            },
        )
        .await;
        assert_eq!(
            receive(node).await,
            ServerMessage::ExecutionEventsAccepted {
                command_id,
                through_execution_sequence,
            }
        );
    }

    #[tokio::test]
    async fn a_stop_is_durable_when_answered_and_delivered_when_the_node_acknowledges_it() {
        let system = TestSystem::start("workspace:stop").await;
        let mut node = system.connect(&system.enroll_node().await).await;
        let mut mac = system.connect(&system.enroll_surface("mac").await).await;
        let command_id = CommandId::new();
        submit(&mut mac, system.task_id, command_id, "Run for a while.").await;
        assert!(matches!(
            receive(&mut node).await,
            ServerMessage::Execute { .. }
        ));

        assert_eq!(
            stop(&mut mac, system.task_id, command_id).await,
            requested(command_id)
        );
        assert_eq!(
            stop_rows(&system),
            vec![(command_id.to_string(), false)],
            "the stop was durable when it was answered"
        );
        assert_eq!(receive(&mut node).await, stop_frame(&system, command_id));

        acknowledge_stop(&mut node, &system, command_id).await;
        barrier(&mut node, &system, command_id).await;
        assert_eq!(stop_rows(&system), vec![(command_id.to_string(), true)]);

        system.stop().await;
    }

    #[tokio::test]
    async fn a_stop_requested_while_the_node_is_offline_follows_its_execution_until_acknowledged() {
        let system = TestSystem::start("workspace:stop-offline").await;
        let node_device = system.enroll_node().await;
        let mut node = system.connect(&node_device).await;
        let mut mac = system.connect(&system.enroll_surface("mac").await).await;
        let command_id = CommandId::new();
        submit(&mut mac, system.task_id, command_id, "Run for a while.").await;
        let delivery = receive(&mut node).await;
        node.close(None).await.expect("disconnect the node");

        assert_eq!(
            stop(&mut mac, system.task_id, command_id).await,
            requested(command_id),
            "a stop is accepted while the node is offline"
        );

        let mut reconnected = system.connect(&node_device).await;
        assert_eq!(receive(&mut reconnected).await, delivery);
        assert_eq!(
            receive(&mut reconnected).await,
            stop_frame(&system, command_id),
            "the stop follows the execution it stops"
        );
        reconnected
            .close(None)
            .await
            .expect("disconnect before acknowledging");

        let mut again = system.connect(&node_device).await;
        assert_eq!(receive(&mut again).await, delivery);
        assert_eq!(
            receive(&mut again).await,
            stop_frame(&system, command_id),
            "an unacknowledged stop is delivered again"
        );
        acknowledge_stop(&mut again, &system, command_id).await;
        barrier(&mut again, &system, command_id).await;
        again
            .close(None)
            .await
            .expect("disconnect after acknowledging");

        // Both pending deliveries are settled: the next activation sends the
        // node nothing before it answers.
        let mut last = system.connect(&node_device).await;
        barrier(&mut last, &system, command_id).await;

        system.stop().await;
    }

    #[tokio::test]
    async fn a_stop_survives_a_coordinator_restart() {
        let mut system = TestSystem::start("workspace:stop-restart").await;
        let node_device = system.enroll_node().await;
        let mut node = system.connect(&node_device).await;
        let mut mac = system.connect(&system.enroll_surface("mac").await).await;
        let command_id = CommandId::new();
        submit(&mut mac, system.task_id, command_id, "Run for a while.").await;
        let delivery = receive(&mut node).await;
        assert_eq!(
            stop(&mut mac, system.task_id, command_id).await,
            requested(command_id)
        );
        drop((node, mac));

        system.restart_coordinator().await;

        let mut node = system.connect(&node_device).await;
        assert_eq!(receive(&mut node).await, delivery);
        assert_eq!(receive(&mut node).await, stop_frame(&system, command_id));

        system.stop().await;
    }

    #[tokio::test]
    async fn repeating_a_stop_returns_the_original_answer_and_keeps_one_row() {
        let system = TestSystem::start("workspace:stop-twice").await;
        let mut node = system.connect(&system.enroll_node().await).await;
        let mut mac = system.connect(&system.enroll_surface("mac").await).await;
        let command_id = CommandId::new();
        submit(&mut mac, system.task_id, command_id, "Run for a while.").await;
        assert!(matches!(
            receive(&mut node).await,
            ServerMessage::Execute { .. }
        ));

        for _ in 0..2 {
            assert_eq!(
                stop(&mut mac, system.task_id, command_id).await,
                requested(command_id)
            );
            assert_eq!(
                receive(&mut node).await,
                stop_frame(&system, command_id),
                "an unacknowledged stop is sent again to the online node"
            );
        }
        assert_eq!(stop_rows(&system).len(), 1);

        // The command ends cancelled. A retry still reads the recorded stop,
        // and an acknowledged stop is not sent again.
        acknowledge_stop(&mut node, &system, command_id).await;
        barrier(&mut node, &system, command_id).await;
        publish(
            &mut node,
            &system,
            command_id,
            ended_with(
                ExecutionId::new(),
                ExecutionTerminal::Cancelled {
                    reason: "agent turn was cancelled".to_owned(),
                },
            ),
        )
        .await;
        assert_eq!(
            stop(&mut mac, system.task_id, command_id).await,
            requested(command_id),
            "a recorded stop outranks the command's present state"
        );
        barrier(&mut node, &system, command_id).await;
        assert_eq!(stop_rows(&system), vec![(command_id.to_string(), true)]);

        system.stop().await;
    }

    #[tokio::test]
    async fn a_stop_for_an_ended_command_records_and_delivers_nothing() {
        let system = TestSystem::start("workspace:stop-late").await;
        let mut node = system.connect(&system.enroll_node().await).await;
        let mut mac = system.connect(&system.enroll_surface("mac").await).await;
        let command_id = CommandId::new();
        submit(&mut mac, system.task_id, command_id, "Finish first.").await;
        execute_and_publish(&system, &mut node, command_id).await;

        assert_eq!(
            stop(&mut mac, system.task_id, command_id).await,
            ServerMessage::StopAccepted {
                request_id: 11,
                command_id,
                outcome: StopOutcome::AlreadyEnded,
            }
        );
        assert!(stop_rows(&system).is_empty());
        barrier(&mut node, &system, command_id).await;

        system.stop().await;
    }

    #[tokio::test]
    async fn a_stop_that_names_nothing_the_principal_owns_is_refused_and_leaves_no_row() {
        let system = TestSystem::start("workspace:stop-refused").await;
        let mut node = system.connect(&system.enroll_node().await).await;
        let mut mac = system.connect(&system.enroll_surface("mac").await).await;
        let command_id = CommandId::new();
        submit(&mut mac, system.task_id, command_id, "Run for a while.").await;
        assert!(matches!(
            receive(&mut node).await,
            ServerMessage::Execute { .. }
        ));
        let second_task = TaskId::new();
        system
            .coordinator
            .create_task(TaskSpec {
                task_id: second_task,
                principal_id: system.principal_id,
                node_id: system.node_id,
                target: TargetRef::new("workspace:second"),
            })
            .await
            .expect("create a second task");
        let stranger_device = system
            .enroll(PeerIdentity::Surface {
                principal_id: PrincipalId::new(),
                surface: SurfaceRef::new("stranger"),
            })
            .await;
        let mut stranger = system.connect(&stranger_device).await;

        let replies = [
            stop(&mut mac, system.task_id, CommandId::new()).await,
            stop(&mut mac, second_task, command_id).await,
            stop(&mut stranger, system.task_id, command_id).await,
        ];
        for reply in replies {
            assert!(
                matches!(
                    reply,
                    ServerMessage::Error {
                        request_id: Some(11),
                        code: ErrorCode::NotFound,
                        ..
                    }
                ),
                "{reply:?}"
            );
        }
        assert!(stop_rows(&system).is_empty());
        barrier(&mut node, &system, command_id).await;

        system.stop().await;
    }

    /// A stop is recorded by `admit_stop` and nothing else, so every way into
    /// the recorded state checks the command and its task first.
    #[test]
    fn only_admit_stop_records_a_stop() {
        let sources = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let writers: Vec<String> = std::fs::read_dir(&sources)
            .expect("read the coordinator sources")
            .map(|entry| entry.expect("source entry").path())
            .filter(|path| path.extension().is_some_and(|extension| extension == "rs"))
            .filter(|path| {
                std::fs::read_to_string(path)
                    .expect("read a coordinator source")
                    .contains("INSERT INTO stop_requests")
            })
            .map(|path| {
                path.file_name()
                    .expect("file name")
                    .to_string_lossy()
                    .into_owned()
            })
            .collect();
        assert_eq!(writers, ["command_stop.rs"]);
    }

    #[tokio::test]
    async fn only_the_tasks_node_acknowledges_a_stop() {
        let system = TestSystem::start("workspace:stop-owner").await;
        let mut owner = system.connect(&system.enroll_node().await).await;
        let mut mac = system.connect(&system.enroll_surface("mac").await).await;
        let command_id = CommandId::new();
        submit(&mut mac, system.task_id, command_id, "Run for a while.").await;
        assert!(matches!(
            receive(&mut owner).await,
            ServerMessage::Execute { .. }
        ));
        assert_eq!(
            stop(&mut mac, system.task_id, command_id).await,
            requested(command_id)
        );
        assert_eq!(receive(&mut owner).await, stop_frame(&system, command_id));

        let attacker_device = system
            .enroll(PeerIdentity::Node {
                node_id: NodeId::new(),
            })
            .await;
        let mut attacker = system.connect(&attacker_device).await;
        acknowledge_stop(&mut attacker, &system, command_id).await;
        assert!(matches!(
            receive(&mut attacker).await,
            ServerMessage::Error {
                code: ErrorCode::InvalidMessage,
                ..
            }
        ));
        assert_eq!(stop_rows(&system), vec![(command_id.to_string(), false)]);

        // An acknowledgement that names another task confirms nothing here.
        send(
            &mut owner,
            &ClientMessage::AcknowledgeStop {
                task_id: TaskId::new(),
                command_id,
            },
        )
        .await;
        barrier(&mut owner, &system, command_id).await;
        assert_eq!(stop_rows(&system), vec![(command_id.to_string(), false)]);

        system.stop().await;
    }
}

mod live_activity {
    use renoa_protocol::LiveActivity;

    use super::*;

    async fn attach(surface: &mut Socket, task_id: TaskId, live: bool) {
        send(
            surface,
            &ClientMessage::Attach {
                request_id: 1,
                task_id,
                after_sequence: None,
                live,
            },
        )
        .await;
        assert!(matches!(
            receive(surface).await,
            ServerMessage::Attached { request_id: 1, .. }
        ));
    }

    async fn publish_live(
        node: &mut Socket,
        system: &TestSystem,
        command_id: CommandId,
        activity: LiveActivity,
    ) {
        send(
            node,
            &ClientMessage::PublishLiveActivity {
                task_id: system.task_id,
                command_id,
                activity,
            },
        )
        .await;
    }

    fn delta(sequence: u64, text: &str) -> LiveActivity {
        LiveActivity::TextDelta {
            sequence,
            text: text.to_owned(),
        }
    }

    #[tokio::test]
    async fn only_a_live_attached_surface_receives_frames_and_nothing_is_stored() {
        let system = TestSystem::start("workspace:live").await;
        let mut node = system.connect(&system.enroll_node().await).await;
        let mut live = system.connect(&system.enroll_surface("phone").await).await;
        let mut plain = system.connect(&system.enroll_surface("desk").await).await;
        let mut sender = system.connect(&system.enroll_surface("sender").await).await;
        attach(&mut live, system.task_id, true).await;
        attach(&mut plain, system.task_id, false).await;
        let command_id = CommandId::new();
        submit(&mut sender, system.task_id, command_id, "Write something.").await;
        let (_, transcript) = execute(&system, &mut node, command_id).await;

        publish_live(&mut node, &system, command_id, delta(0, "Hel")).await;
        publish_live(&mut node, &system, command_id, delta(1, "lo")).await;
        publish_live(&mut node, &system, command_id, LiveActivity::TextReset).await;
        send(
            &mut node,
            &ClientMessage::PublishExecutionEvents {
                task_id: system.task_id,
                command_id,
                events: transcript.events,
            },
        )
        .await;

        let mut frames = Vec::new();
        let mut durable = 0;
        while frames.len() < 3 || durable < 5 {
            match receive(&mut live).await {
                ServerMessage::LiveActivity {
                    task_id,
                    command_id: cause,
                    activity,
                } => {
                    assert_eq!((task_id, cause), (system.task_id, command_id));
                    frames.push(activity);
                }
                ServerMessage::TaskEvent { .. } => durable += 1,
                other => panic!("unexpected message {other:?}"),
            }
        }
        assert_eq!(
            frames,
            vec![delta(0, "Hel"), delta(1, "lo"), LiveActivity::TextReset],
            "frames arrive in the order the node published them"
        );

        // The surface that did not ask sees the five journal events and then
        // answers a request; the frames were published before the events, so
        // one forwarded to it would have arrived first.
        for _ in 0..5 {
            assert!(
                matches!(receive(&mut plain).await, ServerMessage::TaskEvent { .. }),
                "a surface that did not attach live received a live frame"
            );
        }
        send(&mut plain, &ClientMessage::ListTasks { request_id: 9 }).await;
        assert!(matches!(
            receive(&mut plain).await,
            ServerMessage::TaskList { request_id: 9, .. }
        ));
        let stored: i64 = Connection::open(system.files.path().join("control.sqlite"))
            .expect("open coordinator store")
            .query_row("SELECT count(*) FROM task_events", [], |row| row.get(0))
            .expect("count the journal");
        assert_eq!(stored, 5, "the command and its four execution events");

        system.stop().await;
    }

    #[tokio::test]
    async fn a_frame_from_a_node_that_does_not_own_the_task_is_not_relayed() {
        let system = TestSystem::start("workspace:live-owner").await;
        let mut owner = system.connect(&system.enroll_node().await).await;
        let mut phone = system.connect(&system.enroll_surface("phone").await).await;
        attach(&mut phone, system.task_id, true).await;
        let command_id = CommandId::new();
        let stranger_device = system
            .enroll(PeerIdentity::Node {
                node_id: NodeId::new(),
            })
            .await;
        let mut stranger = system.connect(&stranger_device).await;

        publish_live(&mut stranger, &system, command_id, delta(0, "forged")).await;
        // The stranger's frames are handled in order: its refused
        // acknowledgement answers only after the forged frame was handled.
        send(
            &mut stranger,
            &ClientMessage::AcknowledgeStop {
                task_id: system.task_id,
                command_id,
            },
        )
        .await;
        assert!(matches!(
            receive(&mut stranger).await,
            ServerMessage::Error {
                code: ErrorCode::InvalidMessage,
                ..
            }
        ));
        publish_live(&mut owner, &system, command_id, delta(0, "genuine")).await;

        assert_eq!(
            receive(&mut phone).await,
            ServerMessage::LiveActivity {
                task_id: system.task_id,
                command_id,
                activity: delta(0, "genuine"),
            }
        );

        system.stop().await;
    }
}

mod status {
    use super::*;

    async fn statuses(surface: &mut Socket) -> Vec<TaskStatus> {
        send(surface, &ClientMessage::ListTasks { request_id: 21 }).await;
        let ServerMessage::TaskList { tasks, .. } = receive(surface).await else {
            panic!("expected a task list");
        };
        tasks.into_iter().map(|task| task.status).collect()
    }

    /// The rule the documents state, applied to the journal a surface holds:
    /// the latest command is working until its execution terminates, and the
    /// terminal decides what follows.
    fn status_of(journal: &[TaskEvent]) -> TaskStatus {
        let latest = journal.iter().rev().find_map(|event| match &event.kind {
            TaskEventKind::CommandSubmitted { command } => Some(command.command_id),
            TaskEventKind::ExecutionEvent { .. } => None,
        });
        let Some(latest) = latest else {
            return TaskStatus::Idle;
        };
        journal
            .iter()
            .find_map(|event| match &event.kind {
                TaskEventKind::ExecutionEvent {
                    command_id,
                    event:
                        ExecutionEvent {
                            kind: ExecutionEventKind::ExecutionTerminated { terminal },
                            ..
                        },
                } if *command_id == latest => Some(match terminal {
                    ExecutionTerminal::Failed { .. } => TaskStatus::Failed,
                    ExecutionTerminal::Completed | ExecutionTerminal::Cancelled { .. } => {
                        TaskStatus::Idle
                    }
                }),
                _ => None,
            })
            .unwrap_or(TaskStatus::Working)
    }

    async fn assert_status(expected: TaskStatus, journal: &[TaskEvent], surface: &mut Socket) {
        assert_eq!(statuses(surface).await, vec![expected], "the directory");
        assert_eq!(status_of(journal), expected, "the journal rule");
    }

    async fn read(surface: &mut Socket, journal: &mut Vec<TaskEvent>, count: usize) {
        for _ in 0..count {
            let ServerMessage::TaskEvent { event } = receive(surface).await else {
                panic!("expected a task event");
            };
            journal.push(event);
        }
    }

    async fn publish(
        node: &mut Socket,
        system: &TestSystem,
        command_id: CommandId,
        events: Vec<ExecutionEvent>,
    ) {
        send(
            node,
            &ClientMessage::PublishExecutionEvents {
                task_id: system.task_id,
                command_id,
                events,
            },
        )
        .await;
        assert!(matches!(
            receive(node).await,
            ServerMessage::ExecutionEventsAccepted { .. }
        ));
    }

    /// Submits `command_id` and has the node acknowledge it; returns the
    /// transcript that would end it in `terminal`.
    async fn start(
        system: &TestSystem,
        node: &mut Socket,
        mac: &mut Socket,
        command_id: CommandId,
        terminal: ExecutionTerminal,
    ) -> Vec<ExecutionEvent> {
        submit(mac, system.task_id, command_id, "Go.").await;
        execute(system, node, command_id).await;
        ended_with(ExecutionId::new(), terminal)
    }

    #[tokio::test]
    async fn the_directory_reports_the_latest_commands_status_as_the_journal_defines_it() {
        let system = TestSystem::start("workspace:status").await;
        let mut node = system.connect(&system.enroll_node().await).await;
        let mut mac = system.connect(&system.enroll_surface("mac").await).await;
        let mut watcher = system
            .connect(&system.enroll_surface("watcher").await)
            .await;
        let mut journal = Vec::new();
        attach_initial(&mut watcher, system.task_id).await;
        assert_status(TaskStatus::Idle, &journal, &mut mac).await;

        let completed = CommandId::new();
        let events = start(
            &system,
            &mut node,
            &mut mac,
            completed,
            ExecutionTerminal::Completed,
        )
        .await;
        read(&mut watcher, &mut journal, 1).await;
        assert_status(TaskStatus::Working, &journal, &mut mac).await;
        publish(&mut node, &system, completed, events[..2].to_vec()).await;
        read(&mut watcher, &mut journal, 2).await;
        assert_status(TaskStatus::Working, &journal, &mut mac).await;
        publish(&mut node, &system, completed, events[2..].to_vec()).await;
        read(&mut watcher, &mut journal, 1).await;
        assert_status(TaskStatus::Idle, &journal, &mut mac).await;

        let failed = CommandId::new();
        let events = start(
            &system,
            &mut node,
            &mut mac,
            failed,
            ExecutionTerminal::Failed {
                error: "the model is unavailable".to_owned(),
            },
        )
        .await;
        read(&mut watcher, &mut journal, 1).await;
        assert_status(TaskStatus::Working, &journal, &mut mac).await;
        publish(&mut node, &system, failed, events).await;
        read(&mut watcher, &mut journal, 3).await;
        assert_status(TaskStatus::Failed, &journal, &mut mac).await;

        let cancelled = CommandId::new();
        let events = start(
            &system,
            &mut node,
            &mut mac,
            cancelled,
            ExecutionTerminal::Cancelled {
                reason: "agent turn was cancelled".to_owned(),
            },
        )
        .await;
        read(&mut watcher, &mut journal, 1).await;
        assert_status(TaskStatus::Working, &journal, &mut mac).await;
        publish(&mut node, &system, cancelled, events).await;
        read(&mut watcher, &mut journal, 3).await;
        assert_status(TaskStatus::Idle, &journal, &mut mac).await;

        system.stop().await;
    }

    #[tokio::test]
    async fn an_older_command_that_never_settled_does_not_hold_a_task_in_working() {
        let system = TestSystem::start("workspace:status-orphan").await;
        let mut node = system.connect(&system.enroll_node().await).await;
        let mut mac = system.connect(&system.enroll_surface("mac").await).await;

        let orphan = CommandId::new();
        submit(&mut mac, system.task_id, orphan, "Never finishes.").await;
        execute(&system, &mut node, orphan).await;
        assert_eq!(statuses(&mut mac).await, vec![TaskStatus::Working]);

        let later = CommandId::new();
        submit(&mut mac, system.task_id, later, "Finishes.").await;
        execute_and_publish(&system, &mut node, later).await;

        assert_eq!(statuses(&mut mac).await, vec![TaskStatus::Idle]);

        system.stop().await;
    }

    #[tokio::test]
    async fn reading_the_directory_writes_nothing() {
        let system = TestSystem::start("workspace:status-read").await;
        let mut node = system.connect(&system.enroll_node().await).await;
        let mut mac = system.connect(&system.enroll_surface("mac").await).await;
        let command_id = CommandId::new();
        submit(&mut mac, system.task_id, command_id, "Finish.").await;
        execute_and_publish(&system, &mut node, command_id).await;
        // `data_version` moves when another connection commits a change.
        let observer = Connection::open(system.files.path().join("control.sqlite"))
            .expect("open coordinator store");
        let version = |connection: &Connection| -> i64 {
            connection
                .query_row("PRAGMA data_version", [], |row| row.get(0))
                .expect("read data version")
        };
        let before = version(&observer);

        assert_eq!(statuses(&mut mac).await, vec![TaskStatus::Idle]);

        assert_eq!(version(&observer), before);

        system.stop().await;
    }

    fn row_counts(system: &TestSystem) -> Vec<i64> {
        let connection = Connection::open(system.files.path().join("control.sqlite"))
            .expect("open coordinator store");
        [
            "tasks",
            "task_events",
            "commands",
            "execution_event_streams",
            "pending_executions",
        ]
        .map(|table| {
            connection
                .query_row(&format!("SELECT count(*) FROM {table}"), [], |row| {
                    row.get::<_, i64>(0)
                })
                .expect("count rows")
        })
        .to_vec()
    }

    /// How many of the stop table and the command view exist, and the
    /// schema version.
    fn stop_schema(system: &TestSystem) -> (i64, i64) {
        let store = Connection::open(system.files.path().join("control.sqlite"))
            .expect("open coordinator store");
        let objects = store
            .query_row(
                "SELECT count(*) FROM sqlite_master
                 WHERE (type = 'table' AND name = 'stop_requests')
                    OR (type = 'view' AND name = 'command_states')",
                [],
                |row| row.get(0),
            )
            .expect("count the stop table and the command view");
        let version = store
            .query_row("PRAGMA user_version", [], |row| row.get(0))
            .expect("read the schema version");
        (objects, version)
    }

    /// A database as the release before stops left it (schema 12) has neither
    /// the stop table nor the command view. Opening it must create both, keep
    /// its rows, and serve status, stop and deletion through them.
    #[tokio::test]
    async fn a_database_from_before_stops_gains_the_stop_table_and_command_view() {
        let mut system = TestSystem::start("workspace:status-upgrade").await;
        let node_device = system.enroll_node().await;
        let mac_device = system.enroll_surface("mac").await;
        let mut node = system.connect(&node_device).await;
        let mut mac = system.connect(&mac_device).await;
        let settled = CommandId::new();
        submit(&mut mac, system.task_id, settled, "Finish.").await;
        execute_and_publish(&system, &mut node, settled).await;
        let unsettled = CommandId::new();
        submit(&mut mac, system.task_id, unsettled, "Keep going.").await;
        let (_, transcript) = execute(&system, &mut node, unsettled).await;
        let before = row_counts(&system);
        // One task, a settled command with four execution records, and a
        // running command that published none.
        assert_eq!(before, vec![1, 6, 2, 1, 0]);
        drop((node, mac));

        system
            .restart_coordinator_after(|database| {
                database
                    .execute_batch(
                        "DROP VIEW command_states;
                         DROP TABLE stop_requests;
                         PRAGMA user_version = 12;",
                    )
                    .expect("return the database to schema 12");
            })
            .await;

        assert_eq!(stop_schema(&system), (2, 13));
        assert_eq!(row_counts(&system), before, "opening keeps every row");

        let mut node = system.connect(&node_device).await;
        let mut mac = system.connect(&mac_device).await;
        assert_eq!(
            statuses(&mut mac).await,
            vec![TaskStatus::Working],
            "the latest command has not settled"
        );
        send(
            &mut mac,
            &ClientMessage::StopCommand {
                request_id: 7,
                task_id: system.task_id,
                command_id: unsettled,
            },
        )
        .await;
        assert_eq!(
            receive(&mut mac).await,
            ServerMessage::StopAccepted {
                request_id: 7,
                command_id: unsettled,
                outcome: StopOutcome::Requested,
            }
        );
        assert_eq!(
            receive(&mut node).await,
            ServerMessage::Stop {
                task_id: system.task_id,
                command_id: unsettled,
            }
        );
        let through = transcript.events.last().expect("terminal event").sequence;
        send(
            &mut node,
            &ClientMessage::PublishExecutionEvents {
                task_id: system.task_id,
                command_id: unsettled,
                events: transcript.events,
            },
        )
        .await;
        assert_eq!(
            receive(&mut node).await,
            ServerMessage::ExecutionEventsAccepted {
                command_id: unsettled,
                through_execution_sequence: through,
            }
        );
        assert_eq!(statuses(&mut mac).await, vec![TaskStatus::Idle]);

        send(
            &mut mac,
            &ClientMessage::DeleteTask {
                request_id: 8,
                task_id: system.task_id,
            },
        )
        .await;
        assert_eq!(
            receive(&mut mac).await,
            ServerMessage::TaskDeleted {
                request_id: 8,
                task_id: system.task_id,
            }
        );
        assert_eq!(row_counts(&system), vec![0; 5], "nothing is kept");

        system.stop().await;
    }

    /// Whether a command has settled is decided by the `command_states` view
    /// alone: the execution stream's flag is named only by the modules that
    /// write, migrate or define it, and every reader goes through the view.
    #[test]
    fn only_the_view_decides_whether_a_command_settled() {
        const OWNERS: [&str; 3] = [
            "control_schema.rs",
            "event_store.rs",
            "control_migrations.rs",
        ];
        const READERS: [&str; 3] = ["command_stop.rs", "task_deletion.rs", "task_status.rs"];
        let sources = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let source_of = |name: &str| {
            std::fs::read_to_string(sources.join(name)).expect("read a coordinator source")
        };
        for entry in std::fs::read_dir(&sources).expect("read the coordinator sources") {
            let path = entry.expect("source entry").path();
            let name = path
                .file_name()
                .expect("file name")
                .to_string_lossy()
                .into_owned();
            if path.extension().is_none_or(|extension| extension != "rs")
                || OWNERS.contains(&name.as_str())
            {
                continue;
            }
            let source = source_of(&name);
            assert_eq!(
                source.matches("execution_event_streams").count(),
                source
                    .matches("DELETE FROM execution_event_streams")
                    .count(),
                "{name} reads the execution stream table; settled is decided by command_states"
            );
        }
        for name in READERS {
            assert!(
                source_of(name).contains("command_states"),
                "{name} no longer reads command_states"
            );
        }
    }
}

struct TestSystem {
    files: TempDir,
    coordinator: Coordinator,
    url: String,
    task_id: TaskId,
    node_id: NodeId,
    principal_id: PrincipalId,
    target: TargetRef,
    shutdown: CancellationToken,
    server: Option<tokio::task::JoinHandle<()>>,
}

impl TestSystem {
    async fn start(target: &str) -> Self {
        let files = TempDir::new().expect("temporary directory");
        let coordinator =
            Coordinator::open(files.path().join("control.sqlite")).expect("open coordinator store");
        let task_id = TaskId::new();
        let node_id = NodeId::new();
        let principal_id = PrincipalId::new();
        let target = TargetRef::new(target);
        coordinator
            .create_task(TaskSpec {
                task_id,
                principal_id,
                node_id,
                target: target.clone(),
            })
            .await
            .expect("create task");

        let (url, shutdown, server) = spawn_coordinator_server(coordinator.clone()).await;
        Self {
            files,
            coordinator,
            url,
            task_id,
            node_id,
            principal_id,
            target,
            shutdown,
            server: Some(server),
        }
    }

    async fn enroll_node(&self) -> DeviceCredentials {
        self.enroll(PeerIdentity::Node {
            node_id: self.node_id,
        })
        .await
    }

    async fn enroll_surface(&self, name: &str) -> DeviceCredentials {
        self.enroll(PeerIdentity::Surface {
            principal_id: self.principal_id,
            surface: SurfaceRef::new(name),
        })
        .await
    }

    /// Enrolls the system's node with the system's principal as its owner.
    async fn enroll_owned_node(&self) -> DeviceCredentials {
        let token = self
            .coordinator
            .create_node_enrollment(
                self.node_id,
                self.principal_id,
                SystemTime::now() + Duration::from_mins(1),
            )
            .await
            .expect("create owned node enrollment");
        self.claim(token).await
    }

    async fn enroll(&self, peer: PeerIdentity) -> DeviceCredentials {
        let token = self
            .coordinator
            .create_enrollment(peer, SystemTime::now() + Duration::from_mins(1))
            .await
            .expect("create device enrollment");
        self.claim(token).await
    }

    async fn claim(&self, token: EnrollmentToken) -> DeviceCredentials {
        let (mut socket, _) = connect_async(&self.url)
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
            panic!("server should enroll device");
        };
        credentials
    }

    async fn connect(&self, credentials: &DeviceCredentials) -> Socket {
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

    async fn restart_coordinator(&mut self) {
        self.restart_coordinator_after(|_| {}).await;
    }

    /// Restarts the coordinator after `edit` changed its database while no
    /// coordinator was serving it.
    async fn restart_coordinator_after(&mut self, edit: impl FnOnce(&Connection)) {
        self.shutdown.cancel();
        self.server
            .take()
            .expect("running server")
            .await
            .expect("server task");
        let database = self.files.path().join("control.sqlite");
        edit(&Connection::open(&database).expect("open the stopped coordinator's database"));
        self.coordinator = Coordinator::open(database).expect("reopen coordinator store");
        let (url, shutdown, server) = spawn_coordinator_server(self.coordinator.clone()).await;
        self.url = url;
        self.shutdown = shutdown;
        self.server = Some(server);
    }

    async fn stop(mut self) {
        self.shutdown.cancel();
        self.server
            .take()
            .expect("running server")
            .await
            .expect("server task");
    }
}

async fn spawn_coordinator_server(
    coordinator: Coordinator,
) -> (String, CancellationToken, tokio::task::JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind coordinator");
    let address = listener.local_addr().expect("coordinator address");
    let shutdown = CancellationToken::new();
    let server_shutdown = shutdown.clone();
    let server = tokio::spawn(async move {
        coordinator
            .serve(listener, server_shutdown)
            .await
            .expect("serve coordinator");
    });
    (format!("ws://{address}/connect"), shutdown, server)
}

async fn submit(socket: &mut Socket, task_id: TaskId, command_id: CommandId, text: &str) {
    send(
        socket,
        &ClientMessage::Submit {
            request_id: 2,
            task_id,
            command_id,
            input: CommandInput::Text {
                text: text.to_owned(),
                context: None,
                author: renoa_protocol::Author::Principal,
            },
        },
    )
    .await;
    assert_eq!(
        receive(socket).await,
        ServerMessage::CommandAccepted {
            request_id: 2,
            command_id,
        }
    );
}

async fn execute_and_publish(
    system: &TestSystem,
    node: &mut Socket,
    command_id: CommandId,
) -> CommandEnvelope {
    let (command, transcript) = execute(system, node, command_id).await;
    let through_execution_sequence = transcript.events.last().expect("terminal event").sequence;
    send(
        node,
        &ClientMessage::PublishExecutionEvents {
            task_id: system.task_id,
            command_id,
            events: transcript.events,
        },
    )
    .await;
    assert_eq!(
        receive(node).await,
        ServerMessage::ExecutionEventsAccepted {
            command_id,
            through_execution_sequence,
        }
    );
    command
}

async fn execute(
    system: &TestSystem,
    node: &mut Socket,
    command_id: CommandId,
) -> (CommandEnvelope, ExecutionTranscript) {
    let ServerMessage::Execute { task_id, command } = receive(node).await else {
        panic!("node should receive an execution command");
    };
    assert_eq!(task_id, system.task_id);
    let execution_id = ExecutionId::new();
    let transcript = ExecutionTranscript {
        events: vec![
            execution_event(execution_id, 0, ExecutionEventKind::ExecutionStarted),
            execution_event(execution_id, 1, ExecutionEventKind::TurnStarted),
            execution_event(
                execution_id,
                2,
                ExecutionEventKind::AssistantMessage {
                    text: "finished".to_owned(),
                },
            ),
            execution_event(
                execution_id,
                3,
                ExecutionEventKind::ExecutionTerminated {
                    terminal: ExecutionTerminal::Completed,
                },
            ),
        ],
    };
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
    (command, transcript)
}

struct ExecutionTranscript {
    events: Vec<ExecutionEvent>,
}

fn execution_event(
    execution_id: ExecutionId,
    sequence: u64,
    kind: ExecutionEventKind,
) -> ExecutionEvent {
    ExecutionEvent {
        event_id: ExecutionEventId::new(),
        execution_id,
        sequence,
        recorded_at_ms: i64::try_from(sequence).expect("test sequence fits in i64"),
        kind,
    }
}

/// An execution transcript that starts, runs one turn and ends in `terminal`.
fn ended_with(execution_id: ExecutionId, terminal: ExecutionTerminal) -> Vec<ExecutionEvent> {
    vec![
        execution_event(execution_id, 0, ExecutionEventKind::ExecutionStarted),
        execution_event(execution_id, 1, ExecutionEventKind::TurnStarted),
        execution_event(
            execution_id,
            2,
            ExecutionEventKind::ExecutionTerminated { terminal },
        ),
    ]
}

async fn attach_initial(socket: &mut Socket, task_id: TaskId) {
    send(
        socket,
        &ClientMessage::Attach {
            request_id: 1,
            task_id,
            after_sequence: None,
            live: false,
        },
    )
    .await;
    assert_eq!(
        receive(socket).await,
        ServerMessage::Attached {
            request_id: 1,
            task_id,
            through_sequence: None,
        }
    );
}

async fn collect_through_terminal(socket: &mut Socket) -> Vec<TaskEvent> {
    let mut events = Vec::new();
    loop {
        let ServerMessage::TaskEvent { event } = receive(socket).await else {
            continue;
        };
        let terminal = matches!(
            &event.kind,
            TaskEventKind::ExecutionEvent {
                event: ExecutionEvent {
                    kind: ExecutionEventKind::ExecutionTerminated { .. },
                    ..
                },
                ..
            }
        );
        events.push(event);
        if terminal {
            return events;
        }
    }
}

async fn assert_replayed_prefix(
    system: &TestSystem,
    command_id: CommandId,
    prefix: &[ExecutionEvent],
) {
    let mut observer = system
        .connect(&system.enroll_surface("observer").await)
        .await;
    send(
        &mut observer,
        &ClientMessage::Attach {
            request_id: 3,
            task_id: system.task_id,
            after_sequence: None,
            live: false,
        },
    )
    .await;
    assert_eq!(
        receive(&mut observer).await,
        ServerMessage::Attached {
            request_id: 3,
            task_id: system.task_id,
            through_sequence: Some(
                u64::try_from(prefix.len()).expect("event prefix length fits in u64")
            ),
        }
    );
    assert!(matches!(
        receive(&mut observer).await,
        ServerMessage::TaskEvent {
            event: TaskEvent {
                kind: TaskEventKind::CommandSubmitted { command },
                ..
            }
        } if command.command_id == command_id
    ));
    for expected in prefix {
        assert!(matches!(
            receive(&mut observer).await,
            ServerMessage::TaskEvent {
                event: TaskEvent {
                    kind: TaskEventKind::ExecutionEvent {
                        command_id: cause,
                        event,
                    },
                    ..
                }
            } if cause == command_id && event == *expected
        ));
    }
}

async fn send(socket: &mut Socket, message: &ClientMessage) {
    let value = serde_json::to_string(message).expect("serialize client message");
    socket
        .send(Message::Text(value.into()))
        .await
        .expect("send client message");
}

async fn receive(socket: &mut Socket) -> ServerMessage {
    let message = socket
        .next()
        .await
        .expect("server message")
        .expect("valid websocket message");
    let Message::Text(value) = message else {
        panic!("expected text websocket message");
    };
    serde_json::from_str(&value).expect("deserialize server message")
}
