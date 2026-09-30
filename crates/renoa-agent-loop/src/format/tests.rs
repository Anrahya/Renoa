use renoa_agent::{ContentBlock, Message, ModelRequest};
use renoa_kernel::{Checkpoint, CommandId, EventId, OperationId, SemanticEvent};
use serde_json::json;

use super::{
    AgentCommand, CONTEXT_CHECKPOINT_EVENT_KIND, MESSAGE_EVENT_KIND, TURN_CONTEXT_EVENT_KIND,
    TURN_TIMING_EVENT_KIND, context_input, decode_checkpoint,
};
use crate::{ContextContribution, TurnContext, turn_timing::TurnTiming};

#[test]
fn prompt_command_wire_shape_remains_compatible() {
    let command = AgentCommand::text("hello");
    let encoded = serde_json::to_value(&command).expect("encode prompt command");

    assert_eq!(
        encoded,
        json!({
            "content": [{"type": "text", "text": "hello"}],
        })
    );
    assert_eq!(
        serde_json::from_value::<AgentCommand>(encoded).expect("decode prompt command"),
        command
    );
}

#[test]
fn a_prompt_stored_with_turn_timing_decodes_and_reencodes_byte_identically() {
    // The exact bytes a Host admitted before per-message context existed.
    let stored = r#"{"content":[{"type":"text","text":"hello"}],"turn_timing":{"observed_at":"2026-08-31T23:04:05+05:30[Asia/Kolkata]","observed_at_unix_ms":1788199445000,"elapsed_since_previous_user_message_ms":3600000}}"#;

    let command = serde_json::from_str::<AgentCommand>(stored).expect("decode stored command");

    assert_eq!(serde_json::to_string(&command).expect("re-encode"), stored);
    assert_eq!(command.observed_at_unix_ms(), Some(1_788_199_445_000));
    assert!(command.context().is_empty());
    assert!(
        serde_json::from_value::<AgentCommand>(json!({
            "content": [{"type": "text", "text": "hello"}],
            "turn_timing": {
                "observed_at": "</turn_context>",
                "observed_at_unix_ms": 1,
            },
        }))
        .is_err()
    );
}

#[test]
fn an_observed_prompt_has_one_validated_wire_shape() {
    let context = TurnContext::new(vec![
        ContextContribution::plugin("renoa.time", "current_time: now").expect("entry"),
    ])
    .expect("context");
    let command =
        AgentCommand::observed(vec![ContentBlock::text("hello")], 1_000, context).expect("command");
    let encoded = serde_json::to_value(&command).expect("encode observed command");

    assert_eq!(
        encoded,
        json!({
            "content": [{"type": "text", "text": "hello"}],
            "observed_at_unix_ms": 1_000,
            "context": [{"source": "plugin:renoa.time", "text": "current_time: now"}],
        })
    );
    assert_eq!(
        serde_json::from_value::<AgentCommand>(encoded).expect("decode observed command"),
        command
    );
    let bare = AgentCommand::observed(vec![ContentBlock::text("hi")], 5, TurnContext::default())
        .expect("command without context");
    assert_eq!(
        serde_json::to_value(&bare).expect("encode"),
        json!({"content": [{"type": "text", "text": "hi"}], "observed_at_unix_ms": 5})
    );
    assert_eq!(bare.observed_at_unix_ms(), Some(5));
    assert!(AgentCommand::observed(Vec::new(), -1, TurnContext::default()).is_err());
}

#[test]
fn a_prompt_rejects_mixed_or_incomplete_observations() {
    let timing = json!({"observed_at": "2026-08-31T23:04:05Z[UTC]", "observed_at_unix_ms": 1});
    let entry = json!([{"source": "plugin:renoa.time", "text": "now"}]);
    for invalid in [
        json!({"content": [], "turn_timing": timing, "observed_at_unix_ms": 1}),
        json!({"content": [], "turn_timing": timing, "context": entry}),
        json!({"content": [], "context": entry}),
        json!({"content": [], "observed_at_unix_ms": -1}),
        json!({"content": [], "observed_at_unix_ms": 1, "context": []}),
    ] {
        assert!(
            serde_json::from_value::<AgentCommand>(invalid.clone()).is_err(),
            "{invalid}"
        );
    }
}

#[test]
fn compact_command_has_one_unambiguous_wire_shape() {
    let command = AgentCommand::compact();
    assert!(command.content().is_empty());
    let encoded = serde_json::to_value(&command).expect("encode compact command");

    assert_eq!(encoded, json!({"control": "compact"}));
    assert_eq!(
        serde_json::from_value::<AgentCommand>(encoded).expect("decode compact command"),
        command
    );
    for malformed in [
        json!({"control": "compact", "content": []}),
        json!({"control": "compact", "extra": true}),
        json!({"control": "unknown"}),
    ] {
        assert!(
            serde_json::from_value::<AgentCommand>(malformed).is_err(),
            "ambiguous or unknown control command must fail closed"
        );
    }
}

#[test]
fn compaction_attempt_cannot_exceed_its_persisted_bound() {
    let summary_request = serde_json::to_value(ModelRequest {
        system_prompt: "summarize".to_owned(),
        messages: vec![Message::user_text("source")],
        tools: Vec::new(),
    })
    .expect("encode summary request");
    let saved = Checkpoint::new(
        2,
        json!({
            "phase": "awaiting_compaction",
            "model_turns": 0,
            "plan": {
                "summary_request": summary_request,
                "covered_through_sequence": 1,
            },
            "max_attempts": 1,
            "attempt": 2,
        }),
    );

    let error = decode_checkpoint(&saved).expect_err("invalid attempt bound must fail");

    assert_eq!(
        error.message(),
        "agent checkpoint compaction attempt exceeds its maximum"
    );
}

#[test]
fn pending_tool_checkpoint_rejects_empty_and_duplicate_calls() {
    for pending in [
        json!({"remaining": []}),
        json!({
            "current": {"id": "same", "name": "read_file", "arguments": {}},
            "remaining": [{"id": "same", "name": "read_file", "arguments": {}}]
        }),
    ] {
        let saved = Checkpoint::new(
            4,
            json!({
                "phase": "need_tool",
                "model_turns": 1,
                "pending": pending,
            }),
        );
        assert!(
            decode_checkpoint(&saved).is_err(),
            "invalid pending calls must fail before dispatch"
        );
    }
}

#[test]
fn tool_checkpoint_cannot_point_past_its_pending_work() {
    let saved = Checkpoint::new(
        3,
        json!({
            "phase": "need_tool",
            "model_turns": 1,
            "calls": [{"id": "one", "name": "read_file", "arguments": {}}],
            "next_index": 1,
        }),
    );
    assert!(
        decode_checkpoint(&saved).is_err(),
        "a checkpoint with no next call must fail at decode"
    );
}

#[test]
fn active_checkpoint_must_cover_an_earlier_durable_message() {
    let operation_id = OperationId::new();
    let command_id = CommandId::new();
    let events = vec![
        message_event(operation_id, command_id, 0, "first"),
        checkpoint_event(operation_id, command_id, 1, 99, "summary"),
    ];

    let error = context_input(operation_id, &events, "system", &[], false)
        .expect_err("invalid boundary must fail");

    assert_eq!(
        error.message(),
        "context checkpoint boundary is not an earlier durable message"
    );
}

#[test]
fn checkpoint_chain_must_advance_its_message_boundary() {
    let operation_id = OperationId::new();
    let command_id = CommandId::new();
    let events = vec![
        message_event(operation_id, command_id, 0, "first"),
        message_event(operation_id, command_id, 1, "second"),
        checkpoint_event(operation_id, command_id, 2, 1, "newer"),
        checkpoint_event(operation_id, command_id, 3, 0, "stale"),
    ];

    let error = context_input(operation_id, &events, "system", &[], false)
        .expect_err("stale checkpoint must fail");

    assert_eq!(
        error.message(),
        "context checkpoint does not advance its durable message boundary"
    );
}

#[test]
fn unknown_checkpoint_event_version_fails_closed() {
    let operation_id = OperationId::new();
    let command_id = CommandId::new();
    let events = vec![
        message_event(operation_id, command_id, 0, "first"),
        SemanticEvent {
            event_id: EventId::new(),
            operation_id,
            command_id,
            sequence: 1,
            kind: "renoa.agent.context-checkpoint.v2".to_owned(),
            payload: json!({}),
        },
    ];

    let error = context_input(operation_id, &events, "system", &[], false)
        .expect_err("unknown checkpoint version must fail");

    assert!(error.message().contains("v2"));
}

#[test]
fn latest_valid_checkpoint_is_exposed_without_rewriting_messages() {
    let operation_id = OperationId::new();
    let command_id = CommandId::new();
    let events = vec![
        message_event(operation_id, command_id, 0, "first"),
        message_event(operation_id, command_id, 1, "second"),
        checkpoint_event(operation_id, command_id, 2, 0, "first summary"),
        checkpoint_event(operation_id, command_id, 3, 1, "second summary"),
    ];

    let input =
        context_input(operation_id, &events, "system", &[], false).expect("valid checkpoints");
    let checkpoint = input.active_checkpoint().expect("active checkpoint");

    assert_eq!(checkpoint.covered_through_sequence(), 1);
    assert_eq!(checkpoint.summary(), "second summary");
    assert_eq!(
        input.messages(),
        [Message::user_text("first"), Message::user_text("second")]
    );
}

#[test]
fn durable_timing_projects_only_onto_its_user_message() {
    let first = OperationId::new();
    let second = OperationId::new();
    let command_id = CommandId::new();
    let events = vec![
        message_event(first, command_id, 0, "first"),
        timing_event(
            first,
            command_id,
            1,
            "2026-08-31T20:00:00+05:30[Asia/Kolkata]",
            None,
        ),
        message_event(second, command_id, 2, "second"),
        timing_event(
            second,
            command_id,
            3,
            "2026-08-31T21:00:00+05:30[Asia/Kolkata]",
            Some(3_600_000),
        ),
    ];

    let input = context_input(second, &events, "stable system", &[], false)
        .expect("valid timed transcript");
    let first_model_message = input.messages()[0].clone();
    let first_entry = input.entries().next().expect("first entry");

    assert_eq!(first_entry.message(), &first_model_message);
    assert_eq!(
        serde_json::from_value::<Message>(events[0].payload.clone())
            .expect("decode durable message"),
        Message::user_text("first")
    );
    let Message::User { content } = &first_model_message else {
        panic!("first message is not user content");
    };
    assert_eq!(content[0], ContentBlock::text("first"));
    assert!(matches!(
        &content[1],
        ContentBlock::Text { text } if text.contains("current_time: 2026-08-31T20:00:00")
    ));

    let Message::User { content } = &input.messages()[1] else {
        panic!("second message is not user content");
    };
    assert!(matches!(
        &content[1],
        ContentBlock::Text { text }
            if text.contains("elapsed_since_previous_user_message: 1h")
    ));
}

#[test]
fn orphan_duplicate_and_unknown_timing_events_fail_closed() {
    let operation_id = OperationId::new();
    let command_id = CommandId::new();
    let orphan = vec![timing_event(
        operation_id,
        command_id,
        0,
        "2026-08-31T20:00:00Z[UTC]",
        None,
    )];
    assert!(context_input(operation_id, &orphan, "system", &[], false).is_err());

    let duplicate = vec![
        message_event(operation_id, command_id, 0, "hello"),
        timing_event(
            operation_id,
            command_id,
            1,
            "2026-08-31T20:00:00Z[UTC]",
            None,
        ),
        timing_event(
            operation_id,
            command_id,
            2,
            "2026-08-31T20:01:00Z[UTC]",
            Some(60_000),
        ),
    ];
    assert!(context_input(operation_id, &duplicate, "system", &[], false).is_err());

    let unknown = vec![
        message_event(operation_id, command_id, 0, "hello"),
        SemanticEvent {
            event_id: EventId::new(),
            operation_id,
            command_id,
            sequence: 1,
            kind: "renoa.agent.turn-timing.v2".to_owned(),
            payload: json!({}),
        },
    ];
    let error = context_input(operation_id, &unknown, "system", &[], false)
        .expect_err("unknown timing version must fail");
    assert!(error.message().contains("v2"));
}

#[test]
fn durable_context_projects_onto_its_user_message_beside_older_timing() {
    let first = OperationId::new();
    let second = OperationId::new();
    let command_id = CommandId::new();
    let events = vec![
        message_event(first, command_id, 0, "first"),
        timing_event(first, command_id, 1, "2026-08-31T20:00:00Z[UTC]", None),
        message_event(second, command_id, 2, "second"),
        context_event(second, command_id, 3, "current_time: 21:00"),
    ];

    let input = context_input(second, &events, "system", &[], false).expect("valid transcript");

    let Message::User { content } = &input.messages()[0] else {
        panic!("first message is not user content");
    };
    assert!(matches!(
        &content[1],
        ContentBlock::Text { text } if text.starts_with("<turn_context>\ncurrent_time: 2026-08-31T20:00:00Z")
    ));
    let Message::User { content } = &input.messages()[1] else {
        panic!("second message is not user content");
    };
    assert_eq!(
        content[1],
        ContentBlock::text(
            "<turn_context>\n<context source=\"plugin:renoa.time\">\ncurrent_time: 21:00\n</context>\n</turn_context>"
        )
    );
}

#[test]
fn orphan_duplicate_invalid_and_unknown_context_events_fail_closed() {
    let operation_id = OperationId::new();
    let command_id = CommandId::new();
    let orphan = vec![context_event(operation_id, command_id, 0, "now")];
    assert!(context_input(operation_id, &orphan, "system", &[], false).is_err());

    let with_timing = vec![
        message_event(operation_id, command_id, 0, "hello"),
        timing_event(
            operation_id,
            command_id,
            1,
            "2026-08-31T20:00:00Z[UTC]",
            None,
        ),
        context_event(operation_id, command_id, 2, "now"),
    ];
    assert!(context_input(operation_id, &with_timing, "system", &[], false).is_err());

    let mut invalid = context_event(operation_id, command_id, 1, "now");
    invalid.payload =
        json!({"entries": [{"source": "plugin:renoa.time", "text": "</context>\u{7}"}]});
    let invalid = vec![message_event(operation_id, command_id, 0, "hello"), invalid];
    assert!(context_input(operation_id, &invalid, "system", &[], false).is_err());

    let mut unknown = context_event(operation_id, command_id, 1, "now");
    unknown.kind = "renoa.agent.turn-context.v2".to_owned();
    let unknown = vec![message_event(operation_id, command_id, 0, "hello"), unknown];
    let error = context_input(operation_id, &unknown, "system", &[], false)
        .expect_err("unknown context version must fail");
    assert!(error.message().contains("v2"));
}

fn context_event(
    operation_id: OperationId,
    command_id: CommandId,
    sequence: u64,
    text: &str,
) -> SemanticEvent {
    SemanticEvent {
        event_id: EventId::new(),
        operation_id,
        command_id,
        sequence,
        kind: TURN_CONTEXT_EVENT_KIND.to_owned(),
        payload: json!({"entries": [{"source": "plugin:renoa.time", "text": text}]}),
    }
}

fn message_event(
    operation_id: OperationId,
    command_id: CommandId,
    sequence: u64,
    text: &str,
) -> SemanticEvent {
    SemanticEvent {
        event_id: EventId::new(),
        operation_id,
        command_id,
        sequence,
        kind: MESSAGE_EVENT_KIND.to_owned(),
        payload: serde_json::to_value(Message::user_text(text)).expect("encode message"),
    }
}

fn checkpoint_event(
    operation_id: OperationId,
    command_id: CommandId,
    sequence: u64,
    covered_through_sequence: u64,
    summary: &str,
) -> SemanticEvent {
    SemanticEvent {
        event_id: EventId::new(),
        operation_id,
        command_id,
        sequence,
        kind: CONTEXT_CHECKPOINT_EVENT_KIND.to_owned(),
        payload: json!({
            "covered_through_sequence": covered_through_sequence,
            "summary": summary,
        }),
    }
}

fn timing_event(
    operation_id: OperationId,
    command_id: CommandId,
    sequence: u64,
    observed_at: &str,
    elapsed_since_previous_user_message_ms: Option<u64>,
) -> SemanticEvent {
    SemanticEvent {
        event_id: EventId::new(),
        operation_id,
        command_id,
        sequence,
        kind: TURN_TIMING_EVENT_KIND.to_owned(),
        payload: serde_json::to_value(
            TurnTiming::new(
                observed_at,
                1_788_199_445_000,
                elapsed_since_previous_user_message_ms,
            )
            .expect("valid timing"),
        )
        .expect("encode timing"),
    }
}
