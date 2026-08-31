use std::{collections::HashMap, num::NonZeroU32, sync::Arc};

use renoa_agent::{AssistantMetadata, Message, ModelRequest, StopReason};
use renoa_kernel::OperationId;

use super::{CompactingContextStrategy, CompactionLimits, CompactionLimitsError, ContextSizer};
use crate::{ContextInput, ContextPreparation, ContextStrategy, context::ContextOrigin};

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
