use renoa_protocol::{ExecutionEventId, ExecutionEventKind, ExecutionId};
use uuid::Uuid;

use super::*;

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
