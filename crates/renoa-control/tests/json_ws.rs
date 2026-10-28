use renoa_control::{
    ClientMessage, ConnectionTicket, DeviceId, DeviceSummary, ErrorCode, HostMethod,
    JSON_WS_VERSION, NodeId, ServerMessage, StopOutcome, TargetSummary, TaskEvent, TaskEventId,
    TaskEventKind, TaskId, TaskStatus, TaskSummary,
};
use renoa_protocol::{
    AgentMessageKind, AgentOrigin, CommandEnvelope, CommandId, CommandInput, LiveActivity,
    PrincipalId, SurfaceRef, TargetCard, TargetRef,
};
use renoa_protocol::{ExecutionEvent, ExecutionEventId, ExecutionEventKind, ExecutionId};
use serde_json::json;
use uuid::Uuid;

#[test]
fn json_websocket_v14_operation_envelopes_have_expected_shapes() {
    assert_eq!(JSON_WS_VERSION, 14);
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
            "version": 14,
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
fn json_websocket_v14_text_input_carries_context_and_a_guest_author_only_when_present() {
    let task_id = TaskId::from_uuid(Uuid::from_u128(1));
    let command_id = CommandId::from_uuid(Uuid::from_u128(2));
    let submit = ClientMessage::Submit {
        request_id: 7,
        task_id,
        command_id,
        input: CommandInput::Text {
            text: "continue here".to_owned(),
            context: None,
            author: renoa_protocol::Author::Principal,
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
            author: renoa_protocol::Author::Principal,
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
    let guest = ClientMessage::Submit {
        request_id: 10,
        task_id,
        command_id,
        input: CommandInput::Text {
            text: "hi".to_owned(),
            context: None,
            author: renoa_protocol::Author::Guest,
        },
    };
    let guest_json = json!({
        "type": "submit",
        "request_id": 10,
        "task_id": "00000000-0000-0000-0000-000000000001",
        "command_id": "00000000-0000-0000-0000-000000000002",
        "input": {"type": "text", "text": "hi", "author": "guest"}
    });
    assert_eq!(
        serde_json::to_value(&guest).expect("serialize a guest's text"),
        guest_json
    );
    assert_eq!(
        serde_json::from_value::<ClientMessage>(guest_json).expect("deserialize a guest's text"),
        guest
    );
    for author in ["owner", "principal"] {
        assert!(
            serde_json::from_value::<ClientMessage>(json!({
                "type": "submit",
                "request_id": 10,
                "task_id": "00000000-0000-0000-0000-000000000001",
                "command_id": "00000000-0000-0000-0000-000000000002",
                "input": {"type": "text", "text": "hi", "author": author}
            }))
            .is_err(),
            "{author:?} is rejected: the principal is never named on the wire"
        );
    }
}

#[test]
fn json_websocket_v14_encodes_task_discovery() {
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
            status: TaskStatus::Working,
        }],
    };
    let response_json = json!({
        "type": "task_list",
        "request_id": 11,
        "tasks": [{
            "taskId": "00000000-0000-0000-0000-000000000001",
            "target": "workspace:renoa",
            "status": "working"
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

/// The frames exactly as `docs/rcp-json-ws-v0.md` shows them, in both
/// directions, so a client written from the document sends and reads these
/// bytes.
#[test]
fn json_websocket_v14_encodes_the_device_key_handshake_and_device_administration() {
    let device_id = DeviceId::from_uuid(Uuid::from_u128(0x40));
    let nonce = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";
    let client_frames = [
        (
            ClientMessage::DeviceProof {
                signature: "MEUCIQ".to_owned(),
            },
            json!({"type": "device_proof", "signature": "MEUCIQ"}),
        ),
        (
            ClientMessage::ListDevices { request_id: 44 },
            json!({"type": "list_devices", "request_id": 44}),
        ),
        (
            ClientMessage::RevokeDevice {
                request_id: 45,
                device_id,
            },
            json!({
                "type": "revoke_device",
                "request_id": 45,
                "device_id": "00000000-0000-0000-0000-000000000040"
            }),
        ),
    ];
    for (message, expected) in client_frames {
        assert_eq!(
            serde_json::to_value(&message).expect("serialize client frame"),
            expected
        );
        assert_eq!(
            serde_json::from_value::<ClientMessage>(expected).expect("deserialize client frame"),
            message
        );
    }

    let server_frames = [
        (
            ServerMessage::DeviceChallenge {
                nonce: nonce.to_owned(),
            },
            json!({"type": "device_challenge", "nonce": nonce}),
        ),
        (
            ServerMessage::DeviceList {
                request_id: 44,
                devices: vec![DeviceSummary {
                    device_id,
                    surface: SurfaceRef::new("android"),
                    label: "Pixel 9".to_owned(),
                    created_at_ms: 1_790_000_000_000,
                    current: true,
                }],
            },
            json!({
                "type": "device_list",
                "request_id": 44,
                "devices": [{
                    "deviceId": "00000000-0000-0000-0000-000000000040",
                    "surface": "android",
                    "label": "Pixel 9",
                    "createdAtMs": 1_790_000_000_000_u64,
                    "current": true
                }]
            }),
        ),
        (
            ServerMessage::DeviceRevoked {
                request_id: 45,
                device_id,
            },
            json!({
                "type": "device_revoked",
                "request_id": 45,
                "device_id": "00000000-0000-0000-0000-000000000040"
            }),
        ),
    ];
    for (message, expected) in server_frames {
        assert_eq!(
            serde_json::to_value(&message).expect("serialize server frame"),
            expected
        );
        assert_eq!(
            serde_json::from_value::<ServerMessage>(expected).expect("deserialize server frame"),
            message
        );
    }
}

#[test]
fn json_websocket_v14_encodes_target_discovery_and_task_opening() {
    let node_id = NodeId::from_uuid(Uuid::from_u128(3));
    let task_id = TaskId::from_uuid(Uuid::from_u128(1));
    let cases = [
        (
            serde_json::to_value(ClientMessage::AdvertiseTargets {
                targets: vec![TargetCard {
                    target: TargetRef::new("agent:alpha"),
                    name: "Alpha".to_owned(),
                }],
            }),
            json!({
                "type": "advertise_targets",
                "targets": [{"target": "agent:alpha", "name": "Alpha"}]
            }),
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
                    name: "Alpha".to_owned(),
                }],
            }),
            json!({
                "type": "target_list",
                "request_id": 12,
                "targets": [{
                    "nodeId": "00000000-0000-0000-0000-000000000003",
                    "target": "agent:alpha",
                    "name": "Alpha"
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
fn json_websocket_v14_encodes_harness_neutral_execution_events() {
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
                author: renoa_protocol::Author::Principal,
            },
            origin: None,
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

#[test]
fn json_websocket_v14_attach_says_whether_it_wants_live_activity() {
    let task_id = TaskId::from_uuid(Uuid::from_u128(1));
    let attach = ClientMessage::Attach {
        request_id: 3,
        task_id,
        after_sequence: Some(12),
        live: true,
    };
    let attach_json = json!({
        "type": "attach",
        "request_id": 3,
        "task_id": "00000000-0000-0000-0000-000000000001",
        "after_sequence": 12,
        "live": true
    });
    assert_eq!(
        serde_json::to_value(&attach).expect("serialize attach"),
        attach_json
    );
    assert_eq!(
        serde_json::from_value::<ClientMessage>(attach_json).expect("deserialize attach"),
        attach
    );
    assert!(
        serde_json::from_value::<ClientMessage>(json!({
            "type": "attach",
            "request_id": 3,
            "task_id": "00000000-0000-0000-0000-000000000001",
            "after_sequence": null
        }))
        .is_err(),
        "a version 13 attach without `live` is not a version 14 attach"
    );
}

#[test]
fn json_websocket_v14_encodes_stopping_a_command() {
    let task_id = TaskId::from_uuid(Uuid::from_u128(1));
    let command_id = CommandId::from_uuid(Uuid::from_u128(2));
    let ids = json!({
        "task_id": "00000000-0000-0000-0000-000000000001",
        "command_id": "00000000-0000-0000-0000-000000000002"
    });
    let cases = [
        (
            serde_json::to_value(ClientMessage::StopCommand {
                request_id: 5,
                task_id,
                command_id,
            }),
            json!({"type": "stop_command", "request_id": 5, "task_id": ids["task_id"], "command_id": ids["command_id"]}),
        ),
        (
            serde_json::to_value(ServerMessage::StopAccepted {
                request_id: 5,
                command_id,
                outcome: StopOutcome::Requested,
            }),
            json!({"type": "stop_accepted", "request_id": 5, "command_id": ids["command_id"], "outcome": "requested"}),
        ),
        (
            serde_json::to_value(ServerMessage::StopAccepted {
                request_id: 6,
                command_id,
                outcome: StopOutcome::AlreadyEnded,
            }),
            json!({"type": "stop_accepted", "request_id": 6, "command_id": ids["command_id"], "outcome": "already_ended"}),
        ),
        (
            serde_json::to_value(ServerMessage::Stop {
                task_id,
                command_id,
            }),
            json!({"type": "stop", "task_id": ids["task_id"], "command_id": ids["command_id"]}),
        ),
        (
            serde_json::to_value(ClientMessage::AcknowledgeStop {
                task_id,
                command_id,
            }),
            json!({"type": "acknowledge_stop", "task_id": ids["task_id"], "command_id": ids["command_id"]}),
        ),
    ];
    for (encoded, expected) in cases {
        assert_eq!(encoded.expect("serialize frame"), expected);
        let decoded = serde_json::from_value::<ServerMessage>(expected.clone())
            .map(|_| ())
            .or_else(|_| serde_json::from_value::<ClientMessage>(expected).map(|_| ()));
        decoded.expect("the frame decodes as the message it encodes");
    }
}

#[test]
fn json_websocket_v14_encodes_live_activity() {
    let task_id = TaskId::from_uuid(Uuid::from_u128(1));
    let command_id = CommandId::from_uuid(Uuid::from_u128(2));
    let published = ClientMessage::PublishLiveActivity {
        task_id,
        command_id,
        activity: LiveActivity::TextDelta {
            sequence: 3,
            text: "Hel".to_owned(),
        },
    };
    let published_json = json!({
        "type": "publish_live_activity",
        "task_id": "00000000-0000-0000-0000-000000000001",
        "command_id": "00000000-0000-0000-0000-000000000002",
        "activity": {"type": "text_delta", "sequence": 3, "text": "Hel"}
    });
    assert_eq!(
        serde_json::to_value(&published).expect("serialize published activity"),
        published_json
    );
    assert_eq!(
        serde_json::from_value::<ClientMessage>(published_json)
            .expect("deserialize published activity"),
        published
    );
    let relayed = ServerMessage::LiveActivity {
        task_id,
        command_id,
        activity: LiveActivity::TextReset,
    };
    let relayed_json = json!({
        "type": "live_activity",
        "task_id": "00000000-0000-0000-0000-000000000001",
        "command_id": "00000000-0000-0000-0000-000000000002",
        "activity": {"type": "text_reset"}
    });
    assert_eq!(
        serde_json::to_value(&relayed).expect("serialize relayed activity"),
        relayed_json
    );
    assert_eq!(
        serde_json::from_value::<ServerMessage>(relayed_json)
            .expect("deserialize relayed activity"),
        relayed
    );
}

#[test]
fn json_websocket_v14_encodes_host_control_calls() {
    let call_id = Uuid::from_u128(9);
    let call = ServerMessage::HostCall {
        call_id,
        principal_id: PrincipalId::from_uuid(Uuid::from_u128(4)),
        method: HostMethod::Post,
        path: "/v1/host/agents".to_owned(),
        body: Some("{\"name\":\"Scout\"}".to_owned()),
    };
    let call_json = json!({
        "type": "host_call",
        "call_id": "00000000-0000-0000-0000-000000000009",
        "principal_id": "00000000-0000-0000-0000-000000000004",
        "method": "POST",
        "path": "/v1/host/agents",
        "body": "{\"name\":\"Scout\"}"
    });
    assert_eq!(
        serde_json::to_value(&call).expect("serialize Host call"),
        call_json
    );
    assert_eq!(
        serde_json::from_value::<ServerMessage>(call_json).expect("deserialize Host call"),
        call
    );
    let read = ServerMessage::HostCall {
        call_id,
        principal_id: PrincipalId::from_uuid(Uuid::from_u128(4)),
        method: HostMethod::Get,
        path: "/v1/host".to_owned(),
        body: None,
    };
    assert_eq!(
        serde_json::to_value(read).expect("serialize Host read")["body"],
        json!(null)
    );

    assert_eq!(
        serde_json::to_value(ClientMessage::ServeHostControl).expect("serialize advertisement"),
        json!({ "type": "serve_host_control" })
    );
    let complete = ClientMessage::CompleteHostCall {
        call_id,
        status: 200,
        body: "{}".to_owned(),
    };
    let complete_json = json!({
        "type": "complete_host_call",
        "call_id": "00000000-0000-0000-0000-000000000009",
        "status": 200,
        "body": "{}"
    });
    assert_eq!(
        serde_json::to_value(&complete).expect("serialize Host answer"),
        complete_json
    );
    assert_eq!(
        serde_json::from_value::<ClientMessage>(complete_json).expect("deserialize Host answer"),
        complete
    );
    for method in ["get", "DELETE", "PATCH"] {
        assert!(
            serde_json::from_value::<HostMethod>(json!(method)).is_err(),
            "{method} is not a Host control method"
        );
    }
}

#[test]
fn json_websocket_v14_encodes_an_agent_message_and_its_origin() {
    let message = ClientMessage::SendAgentMessage {
        request_id: 21,
        from_command: CommandId::from_uuid(Uuid::from_u128(2)),
        message_id: CommandId::from_uuid(Uuid::from_u128(5)),
        node_id: NodeId::from_uuid(Uuid::from_u128(3)),
        target: TargetRef::new("agent:research"),
        text: "Summarise today's notes.".to_owned(),
    };
    let encoded = json!({
        "type": "send_agent_message",
        "request_id": 21,
        "from_command": "00000000-0000-0000-0000-000000000002",
        "message_id": "00000000-0000-0000-0000-000000000005",
        "node_id": "00000000-0000-0000-0000-000000000003",
        "target": "agent:research",
        "text": "Summarise today's notes."
    });
    assert_eq!(
        serde_json::to_value(&message).expect("serialize message"),
        encoded
    );
    assert_eq!(
        serde_json::from_value::<ClientMessage>(encoded).expect("deserialize message"),
        message
    );

    let command = CommandEnvelope {
        command_id: CommandId::from_uuid(Uuid::from_u128(5)),
        principal_id: PrincipalId::from_uuid(Uuid::from_u128(4)),
        surface: SurfaceRef::new("agent"),
        target: TargetRef::new("agent:research"),
        input: CommandInput::from_text("Summarise today's notes."),
        origin: Some(AgentOrigin {
            from_command: CommandId::from_uuid(Uuid::from_u128(2)),
            from: TargetRef::new("agent:desk"),
            name: "Desk".to_owned(),
            depth: 1,
            kind: AgentMessageKind::Request,
        }),
    };
    assert_eq!(
        serde_json::to_value(&command).expect("serialize command"),
        json!({
            "commandId": "00000000-0000-0000-0000-000000000005",
            "principalId": "00000000-0000-0000-0000-000000000004",
            "surface": "agent",
            "target": "agent:research",
            "input": { "type": "text", "text": "Summarise today's notes." },
            "origin": {
                "fromCommand": "00000000-0000-0000-0000-000000000002",
                "from": "agent:desk",
                "name": "Desk",
                "depth": 1,
                "kind": "request"
            }
        })
    );
}
