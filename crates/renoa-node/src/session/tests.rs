use std::path::PathBuf;

use renoa_protocol::{
    CommandInput, ExecutionEvent, ExecutionEventId, ExecutionEventKind, ExecutionId, LiveActivity,
    PrincipalId, SurfaceRef, TargetRef,
};
use tokio::{net::TcpListener, sync::mpsc, time::timeout};
use uuid::Uuid;

use super::{publication::publication_batch, *};
use crate::node_store::{NodeStore, TargetBinding};

/// A coordinator that records what a node sends it, and the node's socket.
async fn recording_coordinator() -> (Socket, mpsc::UnboundedReceiver<ClientMessage>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let address = listener.local_addr().expect("address");
    let (record, received) = mpsc::unbounded_channel();
    tokio::spawn(async move {
        let (stream, _) = listener.accept().await.expect("accept the node");
        let mut server = tokio_tungstenite::accept_async(stream)
            .await
            .expect("handshake");
        while let Some(Ok(Message::Text(json))) = server.next().await {
            let message = serde_json::from_str(&json).expect("a client message");
            let _ = record.send(message);
        }
    });
    let (socket, _) = tokio_tungstenite::connect_async(format!("ws://{address}"))
        .await
        .expect("connect");
    (socket, received)
}

#[tokio::test]
async fn a_frame_queued_before_a_record_is_read_reaches_the_coordinator_before_it() {
    let (mut socket, mut received) = recording_coordinator().await;
    let files = tempfile::tempdir().expect("temporary directory");
    let store = NodeStore::open(files.path().join("node.sqlite")).expect("open node ledger");
    let task_id = TaskId::new();
    let command_id = CommandId::new();
    let command = CommandEnvelope {
        command_id,
        principal_id: PrincipalId::new(),
        surface: SurfaceRef::new("phone"),
        target: TargetRef::new("agent:alpha"),
        input: CommandInput::Text {
            text: "write".to_owned(),
            context: None,
            author: renoa_protocol::Author::Principal,
        },
        origin: None,
    };
    let binding = TargetBinding {
        target: "agent:alpha".to_owned(),
        agent_id: Uuid::new_v4(),
        session_id: Uuid::new_v4(),
        workspace: PathBuf::from("/srv/renoa/alpha"),
    };
    let record = store
        .admit(task_id, command, binding)
        .await
        .expect("admit the command");
    let mut publications = HashMap::new();
    publications.insert(
        command_id,
        Publication {
            record,
            admission_in_flight: false,
            event_in_flight: None,
        },
    );
    // The frame is queued while the loop is busy elsewhere, then the record
    // is read.
    let (live_frames, mut live) = tokio::sync::broadcast::channel(8);
    live_frames
        .send(LiveFrame {
            task_id,
            command_id,
            activity: LiveActivity::TextDelta {
                sequence: 0,
                text: "Hel".to_owned(),
            },
        })
        .expect("queue the frame");

    send_pending(&mut socket, &store, &mut live, &mut publications)
        .await
        .expect("send what is pending");

    let mut kinds = Vec::new();
    while kinds.len() < 3 {
        let message = timeout(Duration::from_secs(5), received.recv())
            .await
            .expect("the node sent the frame and the record")
            .expect("the coordinator stayed connected");
        kinds.push(match message {
            ClientMessage::AcknowledgeExecution { .. } => "acknowledgement",
            ClientMessage::PublishLiveActivity { .. } => "frame",
            ClientMessage::PublishExecutionEvents { .. } => "records",
            other => panic!("the node sent {other:?}"),
        });
    }
    assert_eq!(kinds, ["acknowledgement", "frame", "records"]);
}

#[test]
fn publication_batches_stop_before_the_websocket_limit() {
    let task_id = TaskId::new();
    let command_id = CommandId::new();
    let execution_id = ExecutionId::from_uuid(Uuid::new_v4());
    let events = vec![
        event(execution_id, 0, ExecutionEventKind::ExecutionStarted),
        event(
            execution_id,
            1,
            ExecutionEventKind::AssistantMessage {
                text: "a".repeat(600_000),
            },
        ),
        event(
            execution_id,
            2,
            ExecutionEventKind::AssistantMessage {
                text: "b".repeat(600_000),
            },
        ),
    ];

    let batch = publication_batch(task_id, command_id, events).expect("build batch");

    assert_eq!(batch.len(), 2);
    let encoded = serde_json::to_vec(&ClientMessage::PublishExecutionEvents {
        task_id,
        command_id,
        events: batch,
    })
    .expect("encode batch");
    assert!(encoded.len() <= MAX_APPLICATION_MESSAGE_BYTES);
}

#[test]
fn one_oversized_execution_event_fails_explicitly() {
    let event = event(
        ExecutionId::from_uuid(Uuid::new_v4()),
        1,
        ExecutionEventKind::AssistantMessage {
            text: "x".repeat(MAX_APPLICATION_MESSAGE_BYTES),
        },
    );

    let error = publication_batch(TaskId::new(), CommandId::new(), vec![event])
        .expect_err("oversized event must fail");

    assert!(matches!(error, NodeError::Protocol(_)));
}

fn event(execution_id: ExecutionId, sequence: u64, kind: ExecutionEventKind) -> ExecutionEvent {
    ExecutionEvent {
        event_id: ExecutionEventId::new(),
        execution_id,
        sequence,
        recorded_at_ms: 1,
        kind,
    }
}
