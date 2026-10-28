use std::{
    sync::Arc,
    time::{Duration, SystemTime},
};

use renoa_protocol::{
    CommandEnvelope, CommandId, CommandInput, PrincipalId, SurfaceRef, TargetRef,
};
use tempfile::TempDir;
use tokio::{sync::mpsc, time::timeout};
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use crate::{
    DeviceId, ErrorCode, HostMethod, NodeId, PeerIdentity, ServerMessage, TaskEvent, TaskEventId,
    TaskEventKind, TaskId,
    host_calls::{HOST_CALL_TIMEOUT, HostCallError, HostReply, HostRequest, call, dispatch},
    node_messages::serve_host_control,
    wire::{TASK_BROADCAST_CAPACITY, cleanup_connection, publish_task_event},
};

use super::{Coordinator, NodeConnection, TaskSpec};
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

#[tokio::test(start_paused = true)]
async fn a_host_call_the_node_never_answers_times_out_and_its_late_answer_is_dropped() {
    let files = TempDir::new().expect("temporary directory");
    let coordinator =
        Coordinator::open(files.path().join("control.sqlite")).expect("open coordinator");
    let owner = PrincipalId::from_uuid(Uuid::from_u128(2));
    let node_id = NodeId::from_uuid(Uuid::from_u128(4));
    coordinator
        .create_node_enrollment(node_id, owner, SystemTime::now() + Duration::from_mins(1))
        .await
        .expect("record the node's owner");
    let connection_id = Uuid::from_u128(6);
    let (outgoing, mut sent) = mpsc::channel(1);
    coordinator.state.nodes.lock().await.insert(
        node_id,
        NodeConnection {
            connection_id,
            device_id: DeviceId::from_uuid(Uuid::from_u128(7)),
            outgoing,
            targets: Vec::new(),
            host_control: true,
        },
    );

    let started = tokio::time::Instant::now();
    let outcome = call(
        &coordinator.state,
        owner,
        HostRequest {
            method: HostMethod::Post,
            path: "/v1/host/agents".to_owned(),
            body: None,
        },
    )
    .await;
    assert!(matches!(outcome, Err(HostCallError::TimedOut)));
    assert!(started.elapsed() >= HOST_CALL_TIMEOUT);

    let Some(ServerMessage::HostCall { call_id, .. }) = sent.recv().await else {
        panic!("the node was sent the call before it timed out");
    };
    let late = HostReply {
        status: 200,
        body: "{}".to_owned(),
    };
    assert!(
        !coordinator
            .state
            .host_calls
            .complete(connection_id, call_id, late),
        "a call that timed out takes no answer"
    );
    assert_eq!(coordinator.state.host_calls.len(), 0);
}

fn connection_of_node(
    connection_id: Uuid,
    host_control: bool,
) -> (NodeConnection, mpsc::Receiver<ServerMessage>) {
    let (outgoing, sent) = mpsc::channel(1);
    let connection = NodeConnection {
        connection_id,
        device_id: DeviceId::from_uuid(Uuid::from_u128(7)),
        outgoing,
        targets: Vec::new(),
        host_control,
    };
    (connection, sent)
}

#[tokio::test]
async fn a_connection_a_newer_one_replaced_cannot_mark_the_newer_one_as_serving_host_control() {
    let files = TempDir::new().expect("temporary directory");
    let coordinator =
        Coordinator::open(files.path().join("control.sqlite")).expect("open coordinator");
    let state = &coordinator.state;
    let node_id = NodeId::from_uuid(Uuid::from_u128(4));
    let (replaced, current) = (Uuid::from_u128(6), Uuid::from_u128(8));
    let (connection, _sent) = connection_of_node(current, false);
    state.nodes.lock().await.insert(node_id, connection);

    assert!(!serve_host_control(state, node_id, replaced).await);
    assert!(
        !state.nodes.lock().await[&node_id].host_control,
        "the newer connection never said it serves Host control"
    );
    let unknown = NodeId::from_uuid(Uuid::from_u128(5));
    assert!(!serve_host_control(state, unknown, current).await);

    assert!(serve_host_control(state, node_id, current).await);
    assert!(state.nodes.lock().await[&node_id].host_control);
}

#[tokio::test(start_paused = true)]
async fn a_connection_that_ends_while_a_call_waits_for_the_node_table_ends_that_call() {
    let files = TempDir::new().expect("temporary directory");
    let coordinator =
        Coordinator::open(files.path().join("control.sqlite")).expect("open coordinator");
    let state = Arc::clone(&coordinator.state);
    let owner = PrincipalId::from_uuid(Uuid::from_u128(2));
    let node_id = NodeId::from_uuid(Uuid::from_u128(4));
    let connection_id = Uuid::from_u128(6);
    let (connection, _unread) = connection_of_node(connection_id, true);
    state.nodes.lock().await.insert(node_id, connection);

    // Hold the node table so the call has found nothing yet, let the
    // connection start to end, then release the table to both in the order
    // they asked for it.
    let table = state.nodes.lock().await;
    let calling = tokio::spawn({
        let state = Arc::clone(&state);
        async move {
            let request = HostRequest {
                method: HostMethod::Post,
                path: "/v1/host/agents".to_owned(),
                body: None,
            };
            dispatch(&state, &[node_id], owner, request).await
        }
    });
    tokio::task::yield_now().await;
    let ending = tokio::spawn({
        let state = Arc::clone(&state);
        async move {
            let peer = PeerIdentity::Node { node_id };
            cleanup_connection(&state, &peer, connection_id).await;
        }
    });
    tokio::task::yield_now().await;
    drop(table);

    let outcome = calling.await.expect("call task");
    ending.await.expect("cleanup task");
    assert!(
        matches!(outcome, Err(HostCallError::Disconnected)),
        "a call registered just before its connection ended is ended with it, not left to time out"
    );
    assert_eq!(state.host_calls.len(), 0);
}
