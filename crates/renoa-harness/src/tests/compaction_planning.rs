use std::sync::atomic::{AtomicUsize, Ordering};

use renoa_agent::{AssistantContent, AssistantMetadata, Message, ModelRequest, StopReason};
use uuid::Uuid;

use crate::{
    ContextSizer, OperationId,
    checkpoint::ContextEntry,
    compaction::{CompactionSource, FrozenCompaction},
    compaction_planning::select_plan,
    state::{FrozenRuntime, OperationProgress},
};

#[test]
fn planning_does_not_rebuild_every_possible_context_prefix() {
    let active_operation_id = operation_id();
    let source = CompactionSource {
        progress: OperationProgress {
            runtime: FrozenRuntime {
                revision: "planning-v1".to_owned(),
                system_prompt: "system".to_owned(),
                max_model_attempts: 1,
                max_tool_calls_per_step: 0,
                compaction: Some(policy()),
                tools: Vec::new(),
            },
            model_attempts: 0,
            compaction_attempts: 0,
            force_compaction: false,
        },
        checkpoint: None,
        entries: history(active_operation_id),
    };
    let sizer = CountingSizer::default();

    let plan = select_plan(active_operation_id, &source, policy(), &sizer)
        .expect("plan context")
        .expect("history can be compacted");

    assert!(plan.covered_through_sequence > 100);
    assert!(
        sizer.calls.load(Ordering::Relaxed) < 30,
        "planning must scale logarithmically across safe cuts"
    );
}

fn history(active_operation_id: OperationId) -> Vec<ContextEntry> {
    let mut entries = Vec::new();
    for ordinal in 0_u64..64 {
        let operation_id = operation_id();
        entries.push(ContextEntry {
            operation_id,
            sequence: ordinal * 2,
            message: Message::user_text(format!("request {ordinal}")),
        });
        entries.push(ContextEntry {
            operation_id,
            sequence: ordinal * 2 + 1,
            message: Message::Assistant {
                content: vec![AssistantContent::text(format!("response {ordinal}"))],
                stop_reason: StopReason::Stop,
                usage: None,
                metadata: AssistantMetadata::default(),
            },
        });
    }
    entries.push(ContextEntry {
        operation_id: active_operation_id,
        sequence: 128,
        message: Message::user_text("active request"),
    });
    entries
}

const fn policy() -> FrozenCompaction {
    FrozenCompaction {
        context_window_tokens: 2_000,
        reserved_tokens: 1_000,
        target_input_tokens: 20,
        max_summary_tokens: 10,
        max_attempts: 1,
    }
}

fn operation_id() -> OperationId {
    OperationId::from_uuid(Uuid::new_v4())
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
            let input = serde_json::to_string(request).expect("encode request");
            u64::try_from(input.matches("sequence=").count()).expect("entry count fits u64")
        }
    }
}
