use renoa_agent::{Message, ModelRequest};
use renoa_kernel::{Checkpoint, EventId, OperationId, SemanticEvent};
use serde_json::json;

use super::{CONTEXT_CHECKPOINT_EVENT_KIND, MESSAGE_EVENT_KIND, context_input, decode_checkpoint};

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
    let events = vec![
        message_event(operation_id, 0, "first"),
        checkpoint_event(operation_id, 1, 99, "summary"),
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
    let events = vec![
        message_event(operation_id, 0, "first"),
        message_event(operation_id, 1, "second"),
        checkpoint_event(operation_id, 2, 1, "newer"),
        checkpoint_event(operation_id, 3, 0, "stale"),
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
    let events = vec![
        message_event(operation_id, 0, "first"),
        SemanticEvent {
            event_id: EventId::new(),
            operation_id,
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
    let events = vec![
        message_event(operation_id, 0, "first"),
        message_event(operation_id, 1, "second"),
        checkpoint_event(operation_id, 2, 0, "first summary"),
        checkpoint_event(operation_id, 3, 1, "second summary"),
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

fn message_event(operation_id: OperationId, sequence: u64, text: &str) -> SemanticEvent {
    SemanticEvent {
        event_id: EventId::new(),
        operation_id,
        sequence,
        kind: MESSAGE_EVENT_KIND.to_owned(),
        payload: serde_json::to_value(Message::user_text(text)).expect("encode message"),
    }
}

fn checkpoint_event(
    operation_id: OperationId,
    sequence: u64,
    covered_through_sequence: u64,
    summary: &str,
) -> SemanticEvent {
    SemanticEvent {
        event_id: EventId::new(),
        operation_id,
        sequence,
        kind: CONTEXT_CHECKPOINT_EVENT_KIND.to_owned(),
        payload: json!({
            "covered_through_sequence": covered_through_sequence,
            "summary": summary,
        }),
    }
}
