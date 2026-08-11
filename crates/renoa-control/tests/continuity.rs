use std::time::{Duration, SystemTime};

use futures_util::{SinkExt, StreamExt};
use renoa_control::{
    ClientMessage, Coordinator, DeviceCredentials, ErrorCode, JSON_WS_VERSION, NodeId,
    PeerIdentity, ServerMessage, TaskEvent, TaskEventKind, TaskId, TaskSpec, TaskSummary,
};
use renoa_protocol::{
    CommandEnvelope, CommandId, CommandInput, ExecutionEvent, ExecutionEventId, ExecutionEventKind,
    ExecutionId, ExecutionTerminal, PrincipalId, SurfaceRef, TargetRef,
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
            },
            TaskSummary {
                task_id: second_task_id,
                target: second_target,
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
            Some(TaskEventKind::ExecutionEvent { event })
                if matches!(
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
                    kind: TaskEventKind::ExecutionEvent { event },
                    ..
                }
            } if event == first
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
                }
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
                        kind: TaskEventKind::ExecutionEvent { event },
                        ..
                    }
                } if event == *expected
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

    async fn enroll(&self, peer: PeerIdentity) -> DeviceCredentials {
        let token = self
            .coordinator
            .create_enrollment(peer, SystemTime::now() + Duration::from_mins(1))
            .await
            .expect("create device enrollment");
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
        self.shutdown.cancel();
        self.server
            .take()
            .expect("running server")
            .await
            .expect("server task");
        self.coordinator = Coordinator::open(self.files.path().join("control.sqlite"))
            .expect("reopen coordinator store");
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

async fn attach_initial(socket: &mut Socket, task_id: TaskId) {
    send(
        socket,
        &ClientMessage::Attach {
            request_id: 1,
            task_id,
            after_sequence: None,
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
                }
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
                    kind: TaskEventKind::ExecutionEvent { event },
                    ..
                }
            } if event == *expected
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
