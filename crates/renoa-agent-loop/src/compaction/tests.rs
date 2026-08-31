use std::{
    collections::HashMap,
    num::NonZeroU64,
    sync::atomic::{AtomicUsize, Ordering},
};

use renoa_agent::{
    AssistantContent, AssistantMetadata, ContentBlock, Message, ModelRequest, StopReason, ToolCall,
    ToolResult,
};
use renoa_kernel::OperationId;
use serde_json::json;

use super::{
    CompactionCheckpoint, CompactionLimits, CompactionPlan, CompactionPlanner,
    CompactionPlanningError, ContextSizer, validate_plan,
};
use crate::{ContextInput, TurnTiming, context::ContextOrigin};

#[test]
fn plan_cuts_only_after_a_complete_tool_group_and_preserves_the_active_user() {
    let prior = OperationId::new();
    let active = OperationId::new();
    let mut large_output = "H".repeat(50_000);
    large_output.push_str("TAIL_MARKER");
    let input = context(
        active,
        vec![
            (prior, Message::user_text("prior request")),
            (prior, assistant_text("prior answer")),
            (active, Message::user_text("active request")),
            (
                active,
                assistant(
                    vec![
                        AssistantContent::reasoning("PRIVATE_REASONING", None, false),
                        AssistantContent::tool_call(tool_call("call-1", "read")),
                    ],
                    StopReason::ToolUse,
                ),
            ),
            (active, tool_result("call-1", "read", large_output)),
        ],
    );

    let plan = planner()
        .plan(&input, None, "system", &[], &BudgetSizer)
        .expect("plan valid history")
        .expect("history is compactable");

    assert_eq!(plan.covered_through_sequence(), 4);
    assert!(plan.summary_request().tools.is_empty());
    let encoded = serde_json::to_string(plan.summary_request()).expect("encode summary request");
    assert!(encoded.contains("active request"));
    assert!(encoded.contains("call-1"));
    assert!(encoded.contains("TAIL_MARKER"));
    assert!(encoded.contains("omitted_chars"));
    assert!(encoded.contains("tool_result_sha256"));
    assert!(!encoded.contains("PRIVATE_REASONING"));
    assert!(!encoded.contains(&prior.to_string()));
    assert!(!encoded.contains(&active.to_string()));
    assert!(!encoded.contains("sequence"));
    assert!(
        encoded.len() < 30_000,
        "tool output preview must stay bounded"
    );
}

#[test]
fn summary_input_does_not_expose_host_only_tool_details() {
    let prior = OperationId::new();
    let active = OperationId::new();
    let input = context(
        active,
        vec![
            (prior, Message::user_text("inspect the result")),
            (
                prior,
                assistant(
                    vec![AssistantContent::tool_call(tool_call("call-1", "read"))],
                    StopReason::ToolUse,
                ),
            ),
            (
                prior,
                Message::Tool {
                    result: ToolResult {
                        call_id: "call-1".to_owned(),
                        name: "read".to_owned(),
                        content: vec![ContentBlock::text("model-visible output")],
                        details: Some(json!({"host_only_marker": "must-not-leak"})),
                        is_error: false,
                    },
                },
            ),
            (active, Message::user_text("continue")),
        ],
    );

    let plan = planner()
        .plan(&input, None, "system", &[], &BudgetSizer)
        .expect("plan valid history")
        .expect("history is compactable");
    let encoded = serde_json::to_string(plan.summary_request()).expect("encode summary request");

    assert!(encoded.contains("model-visible output"));
    assert!(!encoded.contains("host_only_marker"));
    assert!(!encoded.contains("must-not-leak"));
}

#[test]
fn summary_input_keeps_the_durable_time_of_each_user_turn() {
    let prior = OperationId::new();
    let active = OperationId::new();
    let timing = TurnTiming::new("2026-08-31T20:00:00Z[UTC]", 1_000, None).expect("valid timing");
    let input = ContextInput::new(
        active,
        vec![
            (ContextOrigin::new(prior, 0), Message::user_text("prior")),
            (ContextOrigin::new(prior, 1), assistant_text("answer")),
            (
                ContextOrigin::new(active, 2),
                Message::user_text("continue"),
            ),
        ],
        &HashMap::from([(prior, timing)]),
        None,
        "system",
        &[],
        false,
    );

    let plan = planner()
        .plan(&input, None, "system", &[], &BudgetSizer)
        .expect("plan valid history")
        .expect("history is compactable");
    let encoded = serde_json::to_string(plan.summary_request()).expect("encode summary request");

    assert!(encoded.contains("current_time: 2026-08-31T20:00:00Z[UTC]"));
}

#[test]
fn plan_chains_from_the_activated_checkpoint_without_resummarizing_it() {
    let first = OperationId::new();
    let second = OperationId::new();
    let active = OperationId::new();
    let input = context(
        active,
        vec![
            (first, Message::user_text("already summarized request")),
            (first, assistant_text("already summarized answer")),
            (second, Message::user_text("newer request")),
            (second, assistant_text("newer answer")),
            (active, Message::user_text("active request")),
        ],
    );

    let plan = planner()
        .plan(
            &input,
            Some(CompactionCheckpoint::new(1, "OLD CHECKPOINT")),
            "system",
            &[],
            &BudgetSizer,
        )
        .expect("plan checkpoint chain")
        .expect("new history is compactable");

    assert_eq!(plan.covered_through_sequence(), 3);
    let encoded = serde_json::to_string(plan.summary_request()).expect("encode summary request");
    assert!(encoded.contains("OLD CHECKPOINT"));
    assert!(encoded.contains("newer request"));
    assert!(encoded.contains("newer answer"));
    assert!(!encoded.contains("already summarized request"));
    assert!(!encoded.contains("already summarized answer"));
    assert!(!encoded.contains("active request"));
}

#[test]
fn malformed_tool_history_fails_before_sizing() {
    let active = OperationId::new();
    let input = context(
        active,
        vec![
            (active, Message::user_text("active request")),
            (active, tool_result("orphan", "read", "unexpected")),
        ],
    );

    assert!(matches!(
        planner().plan(&input, None, "system", &[], &NeverCalledSizer),
        Err(CompactionPlanningError::InvalidHistory(message))
            if message == "tool result has no pending call"
    ));
}

#[test]
fn persisted_plan_boundary_must_remain_a_safe_transcript_cut() {
    let prior = OperationId::new();
    let active = OperationId::new();
    let input = context(
        active,
        vec![
            (prior, Message::user_text("prior request")),
            (
                prior,
                assistant(
                    vec![AssistantContent::tool_call(tool_call("call-1", "read"))],
                    StopReason::ToolUse,
                ),
            ),
            (prior, tool_result("call-1", "read", "result")),
            (active, Message::user_text("active request")),
        ],
    );
    let plan = CompactionPlan {
        summary_request: ModelRequest {
            system_prompt: "summarize".to_owned(),
            messages: vec![Message::user_text("source")],
            tools: Vec::new(),
        },
        covered_through_sequence: 1,
    };

    assert!(matches!(
        validate_plan(&input, &plan),
        Err(CompactionPlanningError::InvalidPlan(message))
            if message == "covered boundary is not a safe transcript cut"
    ));
}

#[test]
fn invalid_active_user_and_checkpoint_fail_before_sizing() {
    let prior = OperationId::new();
    let active = OperationId::new();
    let wrong_anchor = context(
        active,
        vec![
            (prior, Message::user_text("prior request")),
            (active, assistant_text("not a user anchor")),
        ],
    );
    assert!(matches!(
        planner().plan(&wrong_anchor, None, "system", &[], &NeverCalledSizer),
        Err(CompactionPlanningError::InvalidActiveUser)
    ));

    let valid = context(
        active,
        vec![
            (prior, Message::user_text("prior request")),
            (prior, assistant_text("prior answer")),
            (active, Message::user_text("active request")),
        ],
    );
    assert!(matches!(
        planner().plan(
            &valid,
            Some(CompactionCheckpoint::new(0, "  ")),
            "system",
            &[],
            &NeverCalledSizer,
        ),
        Err(CompactionPlanningError::InvalidCheckpoint(message)) if message == "summary is empty"
    ));
    assert!(matches!(
        planner().plan(
            &valid,
            Some(CompactionCheckpoint::new(99, "checkpoint")),
            "system",
            &[],
            &NeverCalledSizer,
        ),
        Err(CompactionPlanningError::InvalidCheckpoint(message))
            if message == "covered sequence is not a durable message"
    ));
}

#[test]
fn an_undispatchable_summary_returns_no_plan() {
    let prior = OperationId::new();
    let active = OperationId::new();
    let input = context(
        active,
        vec![
            (prior, Message::user_text("prior request")),
            (prior, assistant_text("prior answer")),
            (active, Message::user_text("active request")),
        ],
    );

    let plan = planner()
        .plan(&input, None, "system", &[], &OversizedSummarySizer)
        .expect("planning remains valid");

    assert!(plan.is_none());
}

#[test]
fn plan_falls_back_to_the_largest_dispatchable_prefix() {
    let first = OperationId::new();
    let second = OperationId::new();
    let active = OperationId::new();
    let input = context(
        active,
        vec![
            (first, Message::user_text("first request")),
            (first, assistant_text("first answer")),
            (second, Message::user_text("second request")),
            (second, assistant_text("second answer")),
            (active, Message::user_text("active request")),
        ],
    );

    let plan = planner()
        .plan(&input, None, "system", &[], &NoTailFitsSizer)
        .expect("plan valid history")
        .expect("summary input remains dispatchable");

    assert_eq!(plan.covered_through_sequence(), 3);
}

#[test]
fn planning_searches_safe_cuts_logarithmically() {
    let active = OperationId::new();
    let mut entries = Vec::new();
    for ordinal in 0_u64..64 {
        let operation = OperationId::new();
        entries.push((operation, Message::user_text(format!("request {ordinal}"))));
        entries.push((operation, assistant_text(format!("response {ordinal}"))));
    }
    entries.push((active, Message::user_text("active request")));
    let input = context(active, entries);
    let sizer = CountingSizer::default();
    let limits =
        CompactionLimits::new(nz(2_000), 1_000, nz(20), nz(10)).expect("valid performance limits");

    let plan = CompactionPlanner::new(limits)
        .plan(&input, None, "system", &[], &sizer)
        .expect("plan large history")
        .expect("large history is compactable");

    assert!(plan.covered_through_sequence() > 100);
    assert!(
        sizer.calls.load(Ordering::Relaxed) < 30,
        "planning must not rebuild every candidate prefix"
    );
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
        &std::collections::HashMap::new(),
        None,
        "system",
        &[],
        false,
    )
}

fn planner() -> CompactionPlanner {
    let limits =
        CompactionLimits::new(nz(100), 20, nz(50), nz(40)).expect("valid compaction limits");
    CompactionPlanner::new(limits)
}

fn assistant_text(text: impl Into<String>) -> Message {
    assistant(vec![AssistantContent::text(text)], StopReason::Stop)
}

fn assistant(content: Vec<AssistantContent>, stop_reason: StopReason) -> Message {
    Message::Assistant {
        content,
        stop_reason,
        usage: None,
        metadata: AssistantMetadata::default(),
    }
}

fn tool_call(id: &str, name: &str) -> ToolCall {
    ToolCall {
        id: id.to_owned(),
        name: name.to_owned(),
        arguments: json!({ "path": "file.txt" }),
        thought_signature: None,
        namespace: None,
    }
}

fn tool_result(call_id: &str, name: &str, text: impl Into<String>) -> Message {
    Message::Tool {
        result: ToolResult {
            call_id: call_id.to_owned(),
            name: name.to_owned(),
            content: vec![ContentBlock::text(text)],
            details: None,
            is_error: false,
        },
    }
}

fn nz(value: u64) -> NonZeroU64 {
    NonZeroU64::new(value).expect("test value is non-zero")
}

struct BudgetSizer;

impl ContextSizer for BudgetSizer {
    fn estimate_input_tokens(&self, request: &ModelRequest) -> u64 {
        if request.system_prompt == "system" {
            u64::try_from(request.messages.len()).expect("message count fits u64") * 10
        } else {
            10
        }
    }
}

struct OversizedSummarySizer;

impl ContextSizer for OversizedSummarySizer {
    fn estimate_input_tokens(&self, request: &ModelRequest) -> u64 {
        assert_ne!(request.system_prompt, "system");
        81
    }
}

struct NoTailFitsSizer;

impl ContextSizer for NoTailFitsSizer {
    fn estimate_input_tokens(&self, request: &ModelRequest) -> u64 {
        if request.system_prompt == "system" {
            11
        } else {
            10
        }
    }
}

struct NeverCalledSizer;

impl ContextSizer for NeverCalledSizer {
    fn estimate_input_tokens(&self, _request: &ModelRequest) -> u64 {
        panic!("invalid history must fail before sizing")
    }
}

#[derive(Default)]
struct CountingSizer {
    calls: AtomicUsize,
}

impl ContextSizer for CountingSizer {
    fn estimate_input_tokens(&self, request: &ModelRequest) -> u64 {
        self.calls.fetch_add(1, Ordering::Relaxed);
        if request.system_prompt == "system" {
            u64::try_from(request.messages.len()).expect("message count fits u64")
        } else {
            let encoded = serde_json::to_string(request).expect("encode request");
            u64::try_from(encoded.matches("\\\"role\\\"").count()).expect("entry count fits u64")
        }
    }
}
