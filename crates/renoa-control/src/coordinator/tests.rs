use std::{path::Path, sync::Arc, time::Duration};

use renoa_protocol::{
    CommandEnvelope, CommandId, CommandInput, LiveActivity, PrincipalId, SurfaceRef, TargetRef,
};
use tempfile::TempDir;
use tokio::{sync::mpsc, time::timeout};
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use crate::{
    ErrorCode, NodeId, ServerMessage, TaskEvent, TaskEventId, TaskEventKind, TaskId,
    live_activity::{self, LIVE_BROADCAST_CAPACITY},
    wire::{TASK_BROADCAST_CAPACITY, publish_task_event},
};

use super::{Coordinator, TaskSpec};
use crate::surface_operations::attach_surface;

#[tokio::test]
async fn a_lagging_surface_is_told_to_replay() {
    let files = TempDir::new().expect("temporary directory");
    let coordinator =
        Coordinator::open(files.path().join("control.sqlite")).expect("open coordinator");
    let task_id = TaskId::from_uuid(Uuid::from_u128(1));
    let principal_id = PrincipalId::from_uuid(Uuid::from_u128(2));
    let target = TargetRef::new("workspace:lag-test");
    coordinator
        .create_task(TaskSpec {
            task_id,
            principal_id,
            node_id: NodeId::from_uuid(Uuid::from_u128(4)),
            target: target.clone(),
        })
        .await
        .expect("create task");

    let (outgoing, mut responses) = mpsc::channel(1);
    let cancelled = CancellationToken::new();
    attach_surface(
        Arc::clone(&coordinator.state),
        outgoing,
        cancelled.clone(),
        1,
        task_id,
        None,
        principal_id,
        false,
    )
    .await
    .expect("attach surface");
    assert_eq!(
        responses.recv().await,
        Some(ServerMessage::Attached {
            request_id: 1,
            task_id,
            through_sequence: None,
        })
    );

    let command = CommandEnvelope {
        command_id: CommandId::from_uuid(Uuid::from_u128(5)),
        principal_id,
        surface: SurfaceRef::new("lag-test"),
        target,
        input: CommandInput::Text {
            text: "fill the live buffer".to_owned(),
            context: None,
            author: renoa_protocol::Author::Principal,
        },
    };
    // The blocked delivery can hold one queued event and one in-flight event.
    let event_count =
        u64::try_from(TASK_BROADCAST_CAPACITY).expect("broadcast capacity fits in u64") + 3;
    for sequence in 0..event_count {
        publish_task_event(
            &coordinator.state,
            TaskEvent {
                event_id: TaskEventId::from_uuid(Uuid::from_u128(100 + u128::from(sequence))),
                task_id,
                sequence,
                kind: TaskEventKind::CommandSubmitted {
                    command: command.clone(),
                },
            },
        )
        .await;
    }

    loop {
        let response = timeout(Duration::from_secs(1), responses.recv())
            .await
            .expect("coordinator reports a lagging surface")
            .expect("surface response channel remains open");
        match response {
            ServerMessage::TaskEvent { .. } => {}
            ServerMessage::Error {
                request_id, code, ..
            } => {
                assert_eq!(request_id, None);
                assert_eq!(code, ErrorCode::ReplayRequired);
                break;
            }
            other => panic!("unexpected surface response: {other:?}"),
        }
    }
    cancelled.cancel();
}

/// A task with one live-attached surface whose connection queue holds `queue`
/// messages. The test runtime is single-threaded, so the attachment's loop
/// runs only when the test awaits: whatever the test publishes in between is
/// waiting when the loop next runs.
struct LiveAttachment {
    coordinator: Coordinator,
    _files: TempDir,
    task_id: TaskId,
    node_id: NodeId,
    command: CommandEnvelope,
    responses: mpsc::Receiver<ServerMessage>,
    cancelled: CancellationToken,
}

impl LiveAttachment {
    async fn start(queue: usize) -> Self {
        let files = TempDir::new().expect("temporary directory");
        let coordinator =
            Coordinator::open(files.path().join("control.sqlite")).expect("open coordinator");
        let task_id = TaskId::from_uuid(Uuid::from_u128(1));
        let principal_id = PrincipalId::from_uuid(Uuid::from_u128(2));
        let node_id = NodeId::from_uuid(Uuid::from_u128(4));
        let target = TargetRef::new("workspace:live-test");
        coordinator
            .create_task(TaskSpec {
                task_id,
                principal_id,
                node_id,
                target: target.clone(),
            })
            .await
            .expect("create task");
        let (outgoing, responses) = mpsc::channel(queue);
        let cancelled = CancellationToken::new();
        attach_surface(
            Arc::clone(&coordinator.state),
            outgoing,
            cancelled.clone(),
            1,
            task_id,
            None,
            principal_id,
            true,
        )
        .await
        .expect("attach surface");
        let command = write_command(principal_id, target);
        Self {
            coordinator,
            _files: files,
            task_id,
            node_id,
            command,
            responses,
            cancelled,
        }
    }

    /// The task's node publishes one text frame numbered `sequence`. Publishing
    /// is unconstrained, so the runtime's cooperative budget cannot hand the
    /// thread to the attachment's loop halfway through a burst.
    async fn frame(&self, sequence: u64) {
        tokio::task::unconstrained(live_activity::relay(
            &self.coordinator.state,
            self.node_id,
            self.task_id,
            self.command.command_id,
            LiveActivity::TextDelta {
                sequence,
                text: "x".to_owned(),
            },
        ))
        .await;
    }

    /// The coordinator publishes one journal record.
    async fn record(&self, sequence: u64) {
        tokio::task::unconstrained(publish_task_event(
            &self.coordinator.state,
            TaskEvent {
                event_id: TaskEventId::from_uuid(Uuid::from_u128(100 + u128::from(sequence))),
                task_id: self.task_id,
                sequence,
                kind: TaskEventKind::CommandSubmitted {
                    command: self.command.clone(),
                },
            },
        ))
        .await;
    }

    async fn received(&mut self) -> ServerMessage {
        timeout(Duration::from_secs(1), self.responses.recv())
            .await
            .expect("the attachment delivers what it holds")
            .expect("surface response channel remains open")
    }
}

fn delta_sequence(message: &ServerMessage) -> u64 {
    match message {
        ServerMessage::LiveActivity {
            activity: LiveActivity::TextDelta { sequence, .. },
            ..
        } => *sequence,
        other => panic!("expected a text frame, received {other:?}"),
    }
}

#[tokio::test]
async fn a_live_buffer_overrun_skips_frames_and_never_asks_the_surface_to_replay() {
    let mut attachment = LiveAttachment::start(1024).await;
    assert!(matches!(
        attachment.received().await,
        ServerMessage::Attached { .. }
    ));
    let overrun = u64::try_from(LIVE_BROADCAST_CAPACITY).expect("capacity fits in u64") + 44;
    // The loop finds the overrun either through its frame branch or through
    // the frames it forwards ahead of a record. Which one it polls first is
    // random, so every round that does not find it the same way is another
    // chance for a wrong answer to show.
    for round in 0..24 {
        for sequence in 0..overrun {
            attachment.frame(round * 1000 + sequence).await;
        }
        attachment.record(round).await;

        let mut first_frame = None;
        loop {
            match attachment.received().await {
                frame @ ServerMessage::LiveActivity { .. } => {
                    if first_frame.is_none() {
                        first_frame = Some(delta_sequence(&frame));
                    }
                }
                ServerMessage::TaskEvent { .. } => break,
                other => panic!("round {round}: a frame overrun produced {other:?}"),
            }
        }
        assert_eq!(
            first_frame,
            Some(round * 1000 + 44),
            "round {round}: the oldest frames were skipped, so the live buffer overran"
        );
    }
    attachment.cancelled.cancel();
}

#[tokio::test]
async fn a_surface_that_stops_reading_holds_at_most_one_queue_of_frames() {
    // The queue holds the attachment's answer and three frames.
    let mut attachment = LiveAttachment::start(4).await;
    for sequence in 0..50 {
        attachment.frame(sequence).await;
    }
    for _ in 0..4 {
        tokio::task::yield_now().await;
    }
    attachment.record(0).await;

    assert!(matches!(
        attachment.received().await,
        ServerMessage::Attached { .. }
    ));
    let mut frames = Vec::new();
    loop {
        match attachment.received().await {
            frame @ ServerMessage::LiveActivity { .. } => frames.push(delta_sequence(&frame)),
            ServerMessage::TaskEvent { .. } => break,
            other => panic!("unexpected surface response: {other:?}"),
        }
    }
    assert_eq!(
        frames,
        vec![0, 1, 2],
        "a full queue drops frames; none waits behind the surface"
    );
    attachment.cancelled.cancel();
}

#[tokio::test]
async fn a_frame_published_before_a_record_reaches_the_surface_before_it() {
    let mut attachment = LiveAttachment::start(64).await;
    assert!(matches!(
        attachment.received().await,
        ServerMessage::Attached { .. }
    ));
    // Each round leaves a frame and the record published after it waiting
    // together, so the attachment's loop finds both ready.
    for round in 0..64 {
        attachment.frame(round).await;
        attachment.record(round).await;

        assert_eq!(delta_sequence(&attachment.received().await), round);
        assert!(
            matches!(
                attachment.received().await,
                ServerMessage::TaskEvent { event } if event.sequence == round
            ),
            "round {round}: the record overtook the frame published before it"
        );
    }
    attachment.cancelled.cancel();
}

/// The node publishes a frame and then commits a record while a surface is
/// attaching, between its journal subscription and its frame subscription.
/// Whether the attachment replays that record or forwards it live, the frame
/// must not reach the surface after it. The runtime has one SQLite worker:
/// holding the live channels stops the attachment once it has subscribed to
/// the journal, and occupying the worker keeps any suffix read it has not
/// finished from running until the frame and the record are out.
#[test]
fn a_frame_published_before_a_replayed_record_never_follows_it() {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .max_blocking_threads(1)
        .build()
        .expect("test runtime");
    runtime.block_on(async {
        let files = TempDir::new().expect("temporary directory");
        let database = files.path().join("control.sqlite");
        let coordinator = Coordinator::open(&database).expect("open coordinator");
        let task_id = TaskId::from_uuid(Uuid::from_u128(1));
        let principal_id = PrincipalId::from_uuid(Uuid::from_u128(2));
        let node_id = NodeId::from_uuid(Uuid::from_u128(4));
        let target = TargetRef::new("workspace:replay-test");
        coordinator
            .create_task(TaskSpec {
                task_id,
                principal_id,
                node_id,
                target: target.clone(),
            })
            .await
            .expect("create task");
        let command = write_command(principal_id, target);
        let (outgoing, mut responses) = mpsc::channel(64);
        let cancelled = CancellationToken::new();

        let live_channels = coordinator.state.live_channels.lock().await;
        let attachment = tokio::spawn(attach_surface(
            Arc::clone(&coordinator.state),
            outgoing,
            cancelled.clone(),
            1,
            task_id,
            None,
            principal_id,
            true,
        ));
        while !coordinator
            .state
            .task_senders
            .lock()
            .await
            .contains_key(&task_id)
        {
            tokio::time::sleep(Duration::from_millis(1)).await;
        }
        // The attachment waits for the live channels; take the worker first.
        let (release, released) = std::sync::mpsc::channel::<()>();
        let worker = tokio::task::spawn_blocking(move || released.recv());
        drop(live_channels);
        while !coordinator
            .state
            .live_channels
            .lock()
            .await
            .contains_key(&task_id)
        {
            tokio::time::sleep(Duration::from_millis(1)).await;
        }

        // The node's order: the frame, then the record that commits after it.
        live_activity::relay(
            &coordinator.state,
            node_id,
            task_id,
            command.command_id,
            LiveActivity::TextDelta {
                sequence: 0,
                text: "Hel".to_owned(),
            },
        )
        .await;
        let record = commit_record(&database, task_id, &command);
        publish_task_event(&coordinator.state, record).await;
        release.send(()).expect("release the worker");
        worker
            .await
            .expect("worker")
            .expect("the worker was released");
        attachment
            .await
            .expect("attachment")
            .expect("attach surface");
        // A later record ends the delivery: a frame still waiting for the
        // attachment goes out before it, so a late frame cannot hide.
        publish_task_event(
            &coordinator.state,
            TaskEvent {
                event_id: TaskEventId::from_uuid(Uuid::from_u128(101)),
                task_id,
                sequence: 1,
                kind: TaskEventKind::CommandSubmitted { command },
            },
        )
        .await;

        let delivered = deliveries_before_the_second_record(&mut responses).await;
        let record = delivered
            .iter()
            .position(|delivery| *delivery == "record")
            .expect("the record is delivered");
        assert!(
            !delivered[record..].contains(&"frame"),
            "the frame published before the record arrived after it: {delivered:?}"
        );
        cancelled.cancel();
    });
}

fn write_command(principal_id: PrincipalId, target: TargetRef) -> CommandEnvelope {
    CommandEnvelope {
        command_id: CommandId::from_uuid(Uuid::from_u128(5)),
        principal_id,
        surface: SurfaceRef::new("live-test"),
        target,
        input: CommandInput::Text {
            text: "write something".to_owned(),
            context: None,
            author: renoa_protocol::Author::Principal,
        },
    }
}

/// Commits `command`'s submission as the task's next record through the
/// store's own append, without the store's SQLite worker.
fn commit_record(database: &Path, task_id: TaskId, command: &CommandEnvelope) -> TaskEvent {
    let mut connection = crate::control_schema::open_connection(database).expect("open the store");
    let transaction = connection.transaction().expect("begin");
    let record = crate::event_store::append_event(
        &transaction,
        task_id,
        "replay-test",
        &TaskEventKind::CommandSubmitted {
            command: command.clone(),
        },
    )
    .expect("append the record");
    transaction.commit().expect("commit the record");
    record
}

/// What a surface received after `Attached`, in order, up to the record
/// numbered 1.
async fn deliveries_before_the_second_record(
    responses: &mut mpsc::Receiver<ServerMessage>,
) -> Vec<&'static str> {
    let mut next = async || {
        timeout(Duration::from_secs(1), responses.recv())
            .await
            .expect("the attachment delivers what it holds")
            .expect("surface response channel remains open")
    };
    assert!(matches!(next().await, ServerMessage::Attached { .. }));
    let mut delivered = Vec::new();
    loop {
        match next().await {
            ServerMessage::LiveActivity { .. } => delivered.push("frame"),
            ServerMessage::TaskEvent { event } if event.sequence == 0 => delivered.push("record"),
            ServerMessage::TaskEvent { .. } => return delivered,
            other => panic!("unexpected surface response: {other:?}"),
        }
    }
}
