use renoa_control::{
    ClientMessage, ErrorCode, JSON_WS_VERSION, ServerMessage, TaskEvent, TaskEventId,
    TaskEventKind, TaskId, TaskSummary,
};
use renoa_protocol::{
    CommandEnvelope, CommandId, CommandInput, PrincipalId, SurfaceRef, TargetRef,
};
use renoa_protocol::{ExecutionEvent, ExecutionEventId, ExecutionEventKind, ExecutionId};
use serde_json::json;
use uuid::Uuid;

#[test]
fn json_websocket_v8_operation_envelopes_have_expected_shapes() {
    assert_eq!(JSON_WS_VERSION, 8);
    let task_id = TaskId::from_uuid(Uuid::from_u128(1));
    let command_id = CommandId::from_uuid(Uuid::from_u128(2));
    let submit = ClientMessage::Submit {
        request_id: 7,
        task_id,
        command_id,
        input: CommandInput::Text {
            text: "continue here".to_owned(),
        },
    };
    let submit_json = json!({
        "type": "submit",
        "request_id": 7,
        "task_id": "00000000-0000-0000-0000-000000000001",
        "command_id": "00000000-0000-0000-0000-000000000002",
        "input": {
            "type": "text",
            "text": "continue here"
        }
    });
    assert_eq!(
        serde_json::to_value(&submit).expect("serialize submit"),
        submit_json
    );
    assert_eq!(
        serde_json::from_value::<ClientMessage>(submit_json).expect("deserialize submit"),
        submit
    );

    let acknowledgement = ClientMessage::AcknowledgeExecution {
        task_id,
        command_id,
    };
    assert_eq!(
        serde_json::to_value(&acknowledgement).expect("serialize acknowledgement"),
        json!({
            "type": "acknowledge_execution",
            "task_id": "00000000-0000-0000-0000-000000000001",
            "command_id": "00000000-0000-0000-0000-000000000002"
        })
    );

    let error = ServerMessage::Error {
        request_id: Some(7),
        code: ErrorCode::Internal,
        message: "storage unavailable".to_owned(),
    };
    assert_eq!(
        serde_json::to_value(error).expect("serialize error"),
        json!({
            "type": "error",
            "request_id": 7,
            "code": "internal",
            "message": "storage unavailable"
        })
    );
}

#[test]
fn json_websocket_v8_encodes_task_discovery() {
    let task_id = TaskId::from_uuid(Uuid::from_u128(1));
    let request = ClientMessage::ListTasks { request_id: 11 };
    let request_json = json!({
        "type": "list_tasks",
        "request_id": 11
    });
    assert_eq!(
        serde_json::to_value(&request).expect("serialize task discovery"),
        request_json
    );
    assert_eq!(
        serde_json::from_value::<ClientMessage>(request_json).expect("deserialize task discovery"),
        request
    );

    let response = ServerMessage::TaskList {
        request_id: 11,
        tasks: vec![TaskSummary {
            task_id,
            target: TargetRef::new("workspace:renoa"),
        }],
    };
    let response_json = json!({
        "type": "task_list",
        "request_id": 11,
        "tasks": [{
            "taskId": "00000000-0000-0000-0000-000000000001",
            "target": "workspace:renoa"
        }]
    });
    assert_eq!(
        serde_json::to_value(&response).expect("serialize task list"),
        response_json
    );
    assert_eq!(
        serde_json::from_value::<ServerMessage>(response_json).expect("deserialize task list"),
        response
    );
}

#[test]
fn json_websocket_v8_encodes_harness_neutral_execution_events() {
    let task_id = TaskId::from_uuid(Uuid::from_u128(1));
    let command_id = CommandId::from_uuid(Uuid::from_u128(2));
    let message = ClientMessage::PublishExecutionEvents {
        task_id,
        command_id,
        events: vec![ExecutionEvent {
            event_id: ExecutionEventId::from_uuid(Uuid::from_u128(3)),
            execution_id: ExecutionId::from_uuid(Uuid::from_u128(4)),
            sequence: 0,
            recorded_at_ms: 5,
            kind: ExecutionEventKind::ExecutionStarted,
        }],
    };

    assert_eq!(
        serde_json::to_value(message).expect("serialize execution events"),
        json!({
            "type": "publish_execution_events",
            "task_id": "00000000-0000-0000-0000-000000000001",
            "command_id": "00000000-0000-0000-0000-000000000002",
            "events": [{
                "eventId": "00000000-0000-0000-0000-000000000003",
                "executionId": "00000000-0000-0000-0000-000000000004",
                "sequence": 0,
                "recordedAtMs": 5,
                "kind": { "type": "execution_started" }
            }]
        })
    );
}

#[test]
fn execution_task_records_carry_stable_command_causation() {
    let task_id = TaskId::from_uuid(Uuid::from_u128(1));
    let command_id = CommandId::from_uuid(Uuid::from_u128(2));
    let message = ServerMessage::TaskEvent {
        event: TaskEvent {
            event_id: TaskEventId::from_uuid(Uuid::from_u128(3)),
            task_id,
            sequence: 4,
            kind: TaskEventKind::ExecutionEvent {
                command_id,
                event: ExecutionEvent {
                    event_id: ExecutionEventId::from_uuid(Uuid::from_u128(5)),
                    execution_id: ExecutionId::from_uuid(Uuid::from_u128(6)),
                    sequence: 0,
                    recorded_at_ms: 7,
                    kind: ExecutionEventKind::ExecutionStarted,
                },
            },
        },
    };
    let expected = json!({
        "type": "task_event",
        "event": {
            "eventId": "00000000-0000-0000-0000-000000000003",
            "taskId": "00000000-0000-0000-0000-000000000001",
            "sequence": 4,
            "kind": {
                "type": "execution_event",
                "commandId": "00000000-0000-0000-0000-000000000002",
                "event": {
                    "eventId": "00000000-0000-0000-0000-000000000005",
                    "executionId": "00000000-0000-0000-0000-000000000006",
                    "sequence": 0,
                    "recordedAtMs": 7,
                    "kind": { "type": "execution_started" }
                }
            }
        }
    });

    assert_eq!(
        serde_json::to_value(&message).expect("serialize task execution event"),
        expected
    );
    assert_eq!(
        serde_json::from_value::<ServerMessage>(expected)
            .expect("deserialize task execution event"),
        message
    );
}

#[test]
fn execute_delivery_contains_only_continuity_data() {
    let message = ServerMessage::Execute {
        task_id: TaskId::from_uuid(Uuid::from_u128(1)),
        command: CommandEnvelope {
            command_id: CommandId::from_uuid(Uuid::from_u128(2)),
            principal_id: PrincipalId::from_uuid(Uuid::from_u128(4)),
            surface: SurfaceRef::new("phone"),
            target: TargetRef::new("workspace:renoa"),
            input: CommandInput::Text {
                text: "continue".to_owned(),
            },
        },
    };

    assert_eq!(
        serde_json::to_value(message).expect("serialize execution delivery"),
        json!({
            "type": "execute",
            "task_id": "00000000-0000-0000-0000-000000000001",
            "command": {
                "commandId": "00000000-0000-0000-0000-000000000002",
                "principalId": "00000000-0000-0000-0000-000000000004",
                "surface": "phone",
                "target": "workspace:renoa",
                "input": { "type": "text", "text": "continue" }
            }
        })
    );
}
