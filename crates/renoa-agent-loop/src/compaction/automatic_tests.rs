use std::{collections::HashMap, num::NonZeroU32, sync::Arc};

use renoa_agent::{
    AssistantContent, AssistantMetadata, Closing, ContentBlock, Message, ModelRequest, StopReason,
    TokenUsage, ToolCall, ToolResult,
};
use renoa_kernel::OperationId;
use serde_json::json;

use super::{
    CompactingContextStrategy, CompactionLimits, CompactionLimitsError, ContextSizer,
    byte_budget::request_visible_json_bytes,
};
use crate::{
    ContextInput, ContextPreparation, ContextStrategy,
    context::{ContextOrigin, model_visible_messages},
};

#[test]
fn limits_reject_every_invalid_budget_ordering() {
    assert!(matches!(
        CompactionLimits::new(nz(10), 10, nz(5), nz(1)),
        Err(CompactionLimitsError::ReservedTokensExhaustWindow { .. })
    ));
    assert!(matches!(
        CompactionLimits::new(nz(100), 20, nz(80), nz(10)),
        Err(CompactionLimitsError::TargetNotBelowDispatchLimit { .. })
    ));
    assert!(matches!(
        CompactionLimits::new(nz(100), 20, nz(50), nz(50)),
        Err(CompactionLimitsError::SummaryNotBelowTarget { .. })
    ));
    let limits = CompactionLimits::new(nz(100), 20, nz(50), nz(10)).expect("valid default limits");
    assert!(matches!(
        limits.with_automatic_compaction_input_tokens(nz(81)),
        Err(CompactionLimitsError::AutomaticCompactionAboveDispatchLimit { .. })
    ));
    assert!(matches!(
        limits.with_automatic_compaction_input_tokens(nz(50)),
        Err(CompactionLimitsError::TargetNotBelowAutomaticCompaction { .. })
    ));
}

#[test]
fn automatic_trigger_compacts_at_the_exact_configured_input_size() {
    let first = OperationId::new();
    let second = OperationId::new();
    let active = OperationId::new();
    let input = context(
        active,
        vec![
            (first, Message::user_text("first request")),
            (first, assistant_text("first response")),
            (second, Message::user_text("second request")),
            (second, assistant_text("second response")),
            (active, Message::user_text("active request")),
        ],
    );
    let strategy = strategy(Arc::new(MessageCountSizer));

    assert!(matches!(
        strategy.prepare(input).expect("prepare context"),
        ContextPreparation::Compact { .. }
    ));
}

#[test]
fn the_default_policy_allows_the_exact_provider_dispatch_limit() {
    let prior = OperationId::new();
    let active = OperationId::new();
    let limits = CompactionLimits::new(nz(100), 20, nz(30), nz(10)).expect("valid limits");
    let strategy = CompactingContextStrategy::new(
        limits,
        NonZeroU32::new(2).expect("non-zero attempts"),
        Arc::new(DispatchLimitSizer),
    );

    assert!(matches!(
        strategy
            .prepare(context(
                active,
                vec![
                    (prior, Message::user_text("prior request")),
                    (prior, assistant_text("prior response")),
                    (active, Message::user_text("active request")),
                ],
            ))
            .expect("prepare context"),
        ContextPreparation::Model { .. }
    ));
}

#[test]
fn an_uncompactable_active_request_can_use_remaining_provider_capacity() {
    let active = OperationId::new();
    let strategy = strategy(Arc::new(FixedSizer(50)));

    assert!(matches!(
        strategy
            .prepare(context(
                active,
                vec![(active, Message::user_text("one indivisible request"))]
            ))
            .expect("prepare context"),
        ContextPreparation::Model { .. }
    ));
}

fn strategy(sizer: Arc<dyn ContextSizer>) -> CompactingContextStrategy {
    let limits = CompactionLimits::new(nz(100), 20, nz(30), nz(10))
        .expect("valid limits")
        .with_automatic_compaction_input_tokens(nz(50))
        .expect("valid automatic trigger");
    CompactingContextStrategy::new(
        limits,
        NonZeroU32::new(2).expect("non-zero attempts"),
        sizer,
    )
}

fn context(active: OperationId, entries: Vec<(OperationId, Message)>) -> ContextInput {
    ContextInput::new(
        active,
        entries
            .into_iter()
            .enumerate()
            .map(|(sequence, (operation, message))| {
                (
                    ContextOrigin::new(
                        operation,
                        u64::try_from(sequence).expect("test sequence fits u64"),
                    ),
                    message,
                )
            })
            .collect(),
        &HashMap::new(),
        None,
        "system",
        &[],
        false,
    )
}

fn assistant_text(text: impl Into<String>) -> Message {
    Message::Assistant {
        content: vec![renoa_agent::AssistantContent::text(text)],
        stop_reason: StopReason::Stop,
        usage: None,
        metadata: AssistantMetadata::default(),
    }
}

fn nz(value: u64) -> std::num::NonZeroU64 {
    std::num::NonZeroU64::new(value).expect("test value is non-zero")
}

struct MessageCountSizer;

impl ContextSizer for MessageCountSizer {
    fn estimate_input_tokens(&self, request: &ModelRequest) -> u64 {
        if request.system_prompt == "system" {
            u64::try_from(request.messages.len()).expect("message count fits u64") * 10
        } else {
            10
        }
    }
}

struct FixedSizer(u64);

impl ContextSizer for FixedSizer {
    fn estimate_input_tokens(&self, _request: &ModelRequest) -> u64 {
        self.0
    }
}

struct DispatchLimitSizer;

impl ContextSizer for DispatchLimitSizer {
    fn estimate_input_tokens(&self, request: &ModelRequest) -> u64 {
        assert_eq!(request.system_prompt, "system", "must not plan compaction");
        80
    }
}

#[test]
fn byte_budget_validation_rejects_invalid_ordering() {
    let limits = CompactionLimits::new(nz(1_000), 100, nz(400), nz(100)).expect("valid limits");
    assert!(matches!(
        limits.with_request_byte_budget(nz(1_000), nz(1_001), nz(500)),
        Err(CompactionLimitsError::AutomaticBytesAboveDispatchLimit { .. })
    ));
    assert!(matches!(
        limits.with_request_byte_budget(nz(1_000), nz(800), nz(800)),
        Err(CompactionLimitsError::TailBytesNotBelowAutomaticCompaction { .. })
    ));
}

#[test]
fn request_visible_json_bytes_matches_model_visible_serialization() {
    let messages = vec![
        Message::User {
            content: vec![
                ContentBlock::text("look at this"),
                ContentBlock::image("AAAA", "image/jpeg"),
            ],
        },
        Message::Assistant {
            content: vec![AssistantContent::tool_call(ToolCall {
                id: "call-1".to_owned(),
                name: "read_file".to_owned(),
                arguments: json!({"path": "img.png"}),
                thought_signature: Some("sig".to_owned()),
                namespace: None,
            })],
            stop_reason: StopReason::ToolUse,
            usage: Some(TokenUsage {
                input: 10,
                output: 5,
                cache_read: 2,
                cache_write: 1,
            }),
            metadata: AssistantMetadata::default(),
        },
        Message::Tool {
            result: ToolResult {
                call_id: "call-1".to_owned(),
                name: "read_file".to_owned(),
                content: vec![ContentBlock::image("BBBB", "image/jpeg")],
                details: Some(json!({"host_only": "x".repeat(50_000)})),
                is_error: false,
                closing: Some(Closing {
                    message: "done".to_owned(),
                    not_run: "skip".to_owned(),
                }),
            },
        },
    ];
    let req = ModelRequest {
        system_prompt: "system".to_owned(),
        messages: messages.clone(),
        tools: Vec::new(),
    };
    let expected = serde_json::to_vec(&ModelRequest {
        system_prompt: "system".to_owned(),
        messages: model_visible_messages(messages),
        tools: Vec::new(),
    })
    .expect("encode model request")
    .len();
    assert_eq!(
        request_visible_json_bytes(&req).expect("measure visible bytes"),
        u64::try_from(expected).expect("fits u64")
    );
}

#[test]
fn byte_trigger_compacts_low_token_image_sessions_and_bounds_tail_bytes() {
    let first = OperationId::new();
    let second = OperationId::new();
    let third = OperationId::new();
    let active = OperationId::new();
    let limits = CompactionLimits::new(nz(400_000), 10_000, nz(80_000), nz(10_000))
        .expect("valid token limits")
        .with_request_byte_budget(nz(20_000), nz(8_000), nz(3_500))
        .expect("valid byte limits");
    let strategy = CompactingContextStrategy::new(
        limits,
        NonZeroU32::new(2).expect("attempts"),
        Arc::new(FixedSizer(1_000)),
    );
    let image_msg = |tag: &str, data_len: usize| Message::User {
        content: vec![
            ContentBlock::text(tag),
            ContentBlock::image("a".repeat(data_len), "image/jpeg"),
        ],
    };

    let entries = vec![
        (first, image_msg("img-1", 2_500)),
        (first, assistant_text("ok-1")),
        (second, image_msg("img-2", 2_500)),
        (second, assistant_text("ok-2")),
        (third, image_msg("img-3", 2_500)),
        (third, assistant_text("ok-3")),
        (active, Message::user_text("u there?")),
    ];
    let prep = strategy
        .prepare(context(active, entries.clone()))
        .expect("prepare image-heavy context");
    let ContextPreparation::Compact { plan, .. } = prep else {
        panic!("expected byte trigger to initiate compaction: {prep:?}");
    };
    let summary_encoded =
        serde_json::to_string(plan.summary_request()).expect("encode summary request");
    assert!(summary_encoded.contains("image_metadata"));
    assert!(!summary_encoded.contains(&"a".repeat(500)));

    // Retained tail (messages strictly after covered_through_sequence) fits the 3.5 KB tail byte budget.
    let tail_start = usize::try_from(plan.covered_through_sequence() + 1).expect("tail start");
    let tail_req = ModelRequest {
        system_prompt: "system".to_owned(),
        messages: entries[tail_start..]
            .iter()
            .map(|(_, msg)| msg.clone())
            .collect(),
        tools: Vec::new(),
    };
    assert!(
        request_visible_json_bytes(&tail_req).expect("tail bytes") <= 3_500,
        "retained tail must stay within tail_budget_bytes"
    );
}

#[test]
fn host_only_tool_details_do_not_trigger_byte_compaction() {
    let prior = OperationId::new();
    let active = OperationId::new();
    let limits = CompactionLimits::new(nz(400_000), 10_000, nz(80_000), nz(10_000))
        .expect("valid limits")
        .with_request_byte_budget(nz(20_000), nz(8_000), nz(3_500))
        .expect("valid byte limits");
    let strategy = CompactingContextStrategy::new(
        limits,
        NonZeroU32::new(2).expect("attempts"),
        Arc::new(FixedSizer(100)),
    );
    let entries = vec![
        (prior, Message::user_text("inspect")),
        (
            prior,
            Message::Assistant {
                content: vec![AssistantContent::tool_call(ToolCall {
                    id: "c1".to_owned(),
                    name: "read".to_owned(),
                    arguments: json!({}),
                    thought_signature: None,
                    namespace: None,
                })],
                stop_reason: StopReason::ToolUse,
                usage: None,
                metadata: AssistantMetadata::default(),
            },
        ),
        (
            prior,
            Message::Tool {
                result: ToolResult {
                    call_id: "c1".to_owned(),
                    name: "read".to_owned(),
                    content: vec![ContentBlock::text("small")],
                    details: Some(json!({ "blob": "z".repeat(40_000) })),
                    is_error: false,
                    closing: None,
                },
            },
        ),
        (active, Message::user_text("next")),
    ];

    assert!(matches!(
        strategy
            .prepare(context(active, entries))
            .expect("prepare context"),
        ContextPreparation::Model { .. }
    ));
}

#[test]
fn uncompactable_request_over_byte_dispatch_limit_reports_payload_limit_exceeded() {
    let active = OperationId::new();
    let limits = CompactionLimits::new(nz(400_000), 10_000, nz(80_000), nz(10_000))
        .expect("valid limits")
        .with_request_byte_budget(nz(2_000), nz(1_000), nz(500))
        .expect("valid byte limits");
    let strategy = CompactingContextStrategy::new(
        limits,
        NonZeroU32::new(2).expect("attempts"),
        Arc::new(FixedSizer(100)),
    );
    let prep = strategy
        .prepare(context(
            active,
            vec![(active, Message::user_text("x".repeat(5_000)))],
        ))
        .expect("prepare oversized payload");
    assert!(matches!(
        prep,
        ContextPreparation::PayloadLimitExceeded {
            estimated_bytes,
            dispatch_limit_bytes: 2_000,
        } if estimated_bytes > 2_000
    ));
}

#[test]
fn uncompactable_request_between_byte_trigger_and_dispatch_limit_still_dispatches() {
    let active = OperationId::new();
    let limits = CompactionLimits::new(nz(400_000), 10_000, nz(80_000), nz(10_000))
        .expect("valid limits")
        .with_request_byte_budget(nz(4_000), nz(1_000), nz(500))
        .expect("valid byte limits");
    let strategy = CompactingContextStrategy::new(
        limits,
        NonZeroU32::new(2).expect("attempts"),
        Arc::new(FixedSizer(100)),
    );
    let prep = strategy
        .prepare(context(
            active,
            vec![(active, Message::user_text("x".repeat(2_000)))],
        ))
        .expect("prepare active request within dispatch limit");
    assert!(matches!(prep, ContextPreparation::Model { .. }));
}

#[test]
fn automatic_compaction_bytes_equal_to_dispatch_limit_allows_exact_limit() {
    let prior = OperationId::new();
    let active = OperationId::new();
    let entries = vec![
        (
            prior,
            Message::User {
                content: vec![
                    ContentBlock::text("prior"),
                    ContentBlock::image("a".repeat(2_000), "image/jpeg"),
                ],
            },
        ),
        (prior, assistant_text("answer")),
        (active, Message::user_text("next")),
    ];
    let exact_bytes = request_visible_json_bytes(&ModelRequest {
        system_prompt: "system".to_owned(),
        messages: entries.iter().map(|(_, msg)| msg.clone()).collect(),
        tools: Vec::new(),
    })
    .expect("exact byte count");
    let limits = CompactionLimits::new(nz(400_000), 10_000, nz(80_000), nz(10_000))
        .expect("valid limits")
        .with_request_byte_budget(nz(exact_bytes), nz(exact_bytes), nz(exact_bytes - 1))
        .expect("equal trigger and dispatch limits are permitted");
    let strategy = CompactingContextStrategy::new(
        limits,
        NonZeroU32::new(2).expect("attempts"),
        Arc::new(FixedSizer(100)),
    );
    assert!(matches!(
        strategy
            .prepare(context(active, entries))
            .expect("prepare exact dispatch byte limit"),
        ContextPreparation::Model { .. }
    ));
}

#[test]
fn compaction_required_and_explicit_compaction_enforce_byte_dispatch_limits() {
    let prior = OperationId::new();
    let active = OperationId::new();
    let limits = CompactionLimits::new(nz(400_000), 10_000, nz(80_000), nz(10_000))
        .expect("valid limits")
        .with_request_byte_budget(nz(2_000), nz(1_000), nz(500))
        .expect("valid byte limits");
    let strategy = CompactingContextStrategy::new(
        limits,
        NonZeroU32::new(2).expect("attempts"),
        Arc::new(FixedSizer(100)),
    );
    let compactable = ContextInput::new(
        active,
        vec![
            (ContextOrigin::new(prior, 0), Message::user_text("small")),
            (ContextOrigin::new(prior, 1), assistant_text("answer")),
            (ContextOrigin::new(active, 2), Message::user_text("next")),
        ],
        &HashMap::new(),
        None,
        "system",
        &[],
        true,
    );
    assert!(matches!(
        strategy
            .prepare(compactable)
            .expect("compaction_required prepares compact plan"),
        ContextPreparation::Compact { .. }
    ));

    let oversized_explicit = ContextInput::new(
        active,
        vec![
            (
                ContextOrigin::new(prior, 0),
                Message::user_text("x".repeat(5_000)),
            ),
            (ContextOrigin::new(prior, 1), assistant_text("answer")),
        ],
        &HashMap::new(),
        None,
        "system",
        &[],
        false,
    );
    assert!(matches!(
        strategy
            .prepare_explicit_compaction(&oversized_explicit)
            .expect("oversized explicit compaction fails"),
        crate::ExplicitCompactionPreparation::PayloadLimitExceeded {
            dispatch_limit_bytes: 2_000,
            ..
        }
    ));
}
