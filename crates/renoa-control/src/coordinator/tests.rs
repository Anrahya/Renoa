use std::{sync::Arc, time::Duration};

use renoa_protocol::{
    CommandEnvelope, CommandId, CommandInput, PrincipalId, SurfaceRef, TargetRef,
};
use tempfile::TempDir;
use tokio::{sync::mpsc, time::timeout};
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use crate::{
    ErrorCode, NodeId, ServerMessage, TaskEvent, TaskEventId, TaskEventKind, TaskId,
    wire::{TASK_BROADCAST_CAPACITY, publish_task_event},
};

use super::{Coordinator, TaskSpec, attach_surface};

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
