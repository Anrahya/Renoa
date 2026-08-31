use renoa_agent::{ContentBlock, Message, ModelRequest};
use renoa_kernel::{Checkpoint, CommandId, EventId, OperationId, SemanticEvent};
use serde_json::json;

use super::{
    AgentCommand, CONTEXT_CHECKPOINT_EVENT_KIND, MESSAGE_EVENT_KIND, TURN_TIMING_EVENT_KIND,
    context_input, decode_checkpoint,
};
use crate::TurnTiming;

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
fn timed_prompt_has_a_validated_backward_compatible_wire_shape() {
    let timing = TurnTiming::new(
        "2026-08-31T23:04:05+05:30[Asia/Kolkata]",
        1_788_199_445_000,
        Some(3_600_000),
    )
    .expect("valid timing");
    let command = AgentCommand::timed(vec![ContentBlock::text("hello")], timing);
    let encoded = serde_json::to_value(&command).expect("encode timed command");

    assert_eq!(
        encoded,
        json!({
            "content": [{"type": "text", "text": "hello"}],
            "turn_timing": {
                "observed_at": "2026-08-31T23:04:05+05:30[Asia/Kolkata]",
                "observed_at_unix_ms": 1_788_199_445_000_i64,
                "elapsed_since_previous_user_message_ms": 3_600_000,
            },
        })
    );
    assert_eq!(
        serde_json::from_value::<AgentCommand>(encoded).expect("decode timed command"),
        command
    );
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
