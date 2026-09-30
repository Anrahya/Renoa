use renoa_control::{
    ClientMessage, ConnectionTicket, ErrorCode, JSON_WS_VERSION, NodeId, ServerMessage,
    TargetSummary, TaskEvent, TaskEventId, TaskEventKind, TaskId, TaskSummary,
};
use renoa_protocol::{
    CommandEnvelope, CommandId, CommandInput, PrincipalId, SurfaceRef, TargetRef,
};
use renoa_protocol::{ExecutionEvent, ExecutionEventId, ExecutionEventKind, ExecutionId};
use serde_json::json;
use uuid::Uuid;

#[test]
fn json_websocket_v12_operation_envelopes_have_expected_shapes() {
    assert_eq!(JSON_WS_VERSION, 12);
    let ticket: ConnectionTicket = serde_json::from_value(json!(
        "0000000000000000000000000000000000000000000000000000000000000000"
    ))
    .expect("deserialize connection ticket");
    assert_eq!(
        serde_json::to_value(ClientMessage::AuthenticateTicket {
            version: JSON_WS_VERSION,
            ticket,
        })
        .expect("serialize ticket authentication"),
        json!({
            "type": "authenticate_ticket",
            "version": 12,
            "ticket": "0000000000000000000000000000000000000000000000000000000000000000"
        })
    );
    let task_id = TaskId::from_uuid(Uuid::from_u128(1));
    let command_id = CommandId::from_uuid(Uuid::from_u128(2));

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

    let delete = ClientMessage::DeleteTask {
        request_id: 8,
        task_id,
    };
    let delete_json = json!({
        "type": "delete_task",
        "request_id": 8,
        "task_id": "00000000-0000-0000-0000-000000000001"
    });
    assert_eq!(
        serde_json::to_value(&delete).expect("serialize task deletion"),
        delete_json
    );
    assert_eq!(
        serde_json::from_value::<ClientMessage>(delete_json).expect("deserialize task deletion"),
        delete
    );
    assert_eq!(
        serde_json::to_value(ServerMessage::TaskDeleted {
            request_id: 8,
            task_id,
        })
        .expect("serialize task deleted"),
        json!({
            "type": "task_deleted",
            "request_id": 8,
            "task_id": "00000000-0000-0000-0000-000000000001"
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
fn json_websocket_v12_text_input_carries_the_surface_context_only_when_present() {
    let task_id = TaskId::from_uuid(Uuid::from_u128(1));
    let command_id = CommandId::from_uuid(Uuid::from_u128(2));
    let submit = ClientMessage::Submit {
        request_id: 7,
        task_id,
        command_id,
        input: CommandInput::Text {
            text: "continue here".to_owned(),
            context: None,
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
    let placed = ClientMessage::Submit {
        request_id: 9,
        task_id,
        command_id,
        input: CommandInput::Text {
            text: "post it here".to_owned(),
            context: Some("Discord channel #general (5)".to_owned()),
        },
    };
    let placed_json = json!({
        "type": "submit",
        "request_id": 9,
        "task_id": "00000000-0000-0000-0000-000000000001",
        "command_id": "00000000-0000-0000-0000-000000000002",
        "input": {
            "type": "text",
            "text": "post it here",
            "context": "Discord channel #general (5)"
        }
    });
    assert_eq!(
        serde_json::to_value(&placed).expect("serialize submit with context"),
        placed_json
    );
    assert_eq!(
        serde_json::from_value::<ClientMessage>(placed_json).expect("deserialize with context"),
        placed
    );
}

#[test]
fn json_websocket_v12_encodes_task_discovery() {
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
fn json_websocket_v12_encodes_target_discovery_and_task_opening() {
    let node_id = NodeId::from_uuid(Uuid::from_u128(3));
    let task_id = TaskId::from_uuid(Uuid::from_u128(1));
    let cases = [
        (
            serde_json::to_value(ClientMessage::AdvertiseTargets {
                targets: vec![TargetRef::new("agent:alpha")],
            }),
            json!({"type": "advertise_targets", "targets": ["agent:alpha"]}),
        ),
        (
            serde_json::to_value(ClientMessage::ListTargets { request_id: 12 }),
            json!({"type": "list_targets", "request_id": 12}),
        ),
        (
            serde_json::to_value(ClientMessage::OpenTask {
                request_id: 13,
                task_id,
                node_id,
                target: TargetRef::new("agent:alpha"),
            }),
            json!({
                "type": "open_task",
                "request_id": 13,
                "task_id": "00000000-0000-0000-0000-000000000001",
                "node_id": "00000000-0000-0000-0000-000000000003",
                "target": "agent:alpha"
            }),
        ),
        (
            serde_json::to_value(ServerMessage::TargetList {
                request_id: 12,
                targets: vec![TargetSummary {
                    node_id,
                    target: TargetRef::new("agent:alpha"),
                }],
            }),
            json!({
                "type": "target_list",
                "request_id": 12,
                "targets": [{
                    "nodeId": "00000000-0000-0000-0000-000000000003",
                    "target": "agent:alpha"
                }]
            }),
        ),
        (
            serde_json::to_value(ServerMessage::TaskOpened {
                request_id: 13,
                task_id,
            }),
            json!({
                "type": "task_opened",
                "request_id": 13,
                "task_id": "00000000-0000-0000-0000-000000000001"
            }),
        ),
    ];
    for (encoded, expected) in cases {
        assert_eq!(encoded.expect("serialize frame"), expected);
    }
}

#[test]
fn json_websocket_v12_encodes_harness_neutral_execution_events() {
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
                context: None,
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
