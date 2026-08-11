use renoa_protocol::{ExecutionEvent, ExecutionEventId, ExecutionEventKind, ExecutionId};
use serde_json::json;
use uuid::Uuid;

#[test]
fn execution_events_have_a_harness_neutral_wire_shape() {
    let event = ExecutionEvent {
        event_id: ExecutionEventId::from_uuid(Uuid::from_u128(1)),
        execution_id: ExecutionId::from_uuid(Uuid::from_u128(2)),
        sequence: 0,
        recorded_at_ms: 3,
        kind: ExecutionEventKind::ExecutionStarted,
    };

    assert_eq!(
        serde_json::to_value(event).expect("serialize execution event"),
        json!({
            "eventId": "00000000-0000-0000-0000-000000000001",
            "executionId": "00000000-0000-0000-0000-000000000002",
            "sequence": 0,
            "recordedAtMs": 3,
            "kind": { "type": "execution_started" }
        })
    );
}
