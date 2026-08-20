use std::{
    num::{NonZeroU32, NonZeroU64},
    sync::{Arc, Mutex},
};

use renoa_agent::{AssistantContent, Message, ModelRequest, ModelResponse, StopReason, ToolSpec};
use renoa_agent_loop::{
    CompactingContextStrategy, CompactionLimits, CompactionPlan, CompactionValidationError,
    ContextInput, ContextPreparation, ContextProjector, ContextSizer, ContextStrategy,
    ContextStrategyError,
};
use renoa_kernel::Kernel;
use tempfile::tempdir;

use super::compaction_support::{
    Script, ScriptedModel, ThresholdSizer, create_session, nz32, runtime_with_context,
    submit_and_drive, text_response,
};

const PROJECTED_MESSAGE: &str = "Frozen host context.";

#[tokio::test]
async fn external_strategy_can_execute_its_own_typed_compaction_plan() {
    let directory = tempdir().expect("temporary directory");
    let kernel = Kernel::open(directory.path().join("kernel.sqlite3")).expect("open kernel");
    let session_id = create_session(&kernel);
    let requests = Arc::new(Mutex::new(Vec::new()));
    let model = Arc::new(ScriptedModel::new(
        [
            Script::response(text_response("First answer.")),
            Script::response(text_response("Short custom summary.")),
            Script::response(text_response("Second answer.")),
        ],
        Arc::clone(&requests),
    ));
    let runtime = runtime_with_context(
        model,
        Arc::new(CustomCompactionStrategy),
        "external-custom-compaction-v1",
    );

    submit_and_drive(&kernel, session_id, &runtime, "First question.").await;
    submit_and_drive(&kernel, session_id, &runtime, "Second question.").await;

    let requests = requests.lock().expect("request lock");
    assert_eq!(requests.len(), 3);
    assert_eq!(
        requests[1].system_prompt,
        "Summarize the completed work briefly."
    );
    assert!(requests[1].tools.is_empty());
    let final_request = serde_json::to_string(&requests[2]).expect("encode final request");
    assert!(final_request.contains("CUSTOM CHECKPOINT: Short custom summary."));
    assert!(!final_request.contains("First question."));
}

struct CustomCompactionStrategy;

impl ContextStrategy for CustomCompactionStrategy {
    fn project(&self, input: ContextInput) -> Result<Vec<Message>, ContextStrategyError> {
        let checkpoint = input
            .active_checkpoint()
            .map(|checkpoint| checkpoint.summary().to_owned());
        let messages = input.into_messages();
        let Some(summary) = checkpoint else {
            return Ok(messages);
        };
        let active_user = messages
            .last()
            .cloned()
            .ok_or_else(|| ContextStrategyError::new("active user message is missing"))?;
        Ok(vec![
            Message::user_text(format!("CUSTOM CHECKPOINT: {summary}")),
            active_user,
        ])
    }

    fn prepare(&self, input: ContextInput) -> Result<ContextPreparation, ContextStrategyError> {
        if input.active_checkpoint().is_none() && input.messages().len() >= 3 {
            let covered_through_sequence = input
                .entries()
                .nth(1)
                .ok_or_else(|| {
                    ContextStrategyError::new(
                        "completed operation has fewer than two durable messages",
                    )
                })?
                .sequence();
            let plan = CompactionPlan::new(
                ModelRequest {
                    system_prompt: "Summarize the completed work briefly.".to_owned(),
                    messages: input.messages()[..2].to_vec(),
                    tools: Vec::new(),
                },
                covered_through_sequence,
            )
            .map_err(|error| ContextStrategyError::new(error.to_string()))?;
            return Ok(ContextPreparation::Compact {
                plan,
                max_attempts: NonZeroU32::MIN,
            });
        }
        self.project(input)
            .map(|messages| ContextPreparation::Model { messages })
    }

    fn validate_compaction(
        &self,
        _plan: &CompactionPlan,
        response: &ModelResponse,
        _system_prompt: &str,
        _tools: &[ToolSpec],
    ) -> Result<String, CompactionValidationError> {
        if response.stop_reason != StopReason::Stop {
            return Err(CompactionValidationError::new(
                "custom summary did not finish",
            ));
        }
        let [AssistantContent::Text { text, .. }] = response.content.as_slice() else {
            return Err(CompactionValidationError::new(
                "custom summary must contain exactly one text block",
            ));
        };
        Ok(text.clone())
    }
}

#[tokio::test]
async fn projected_retained_tails_are_sized_and_move_the_safe_cut() {
    let limits = CompactionLimits::new(nz64(130), 10, nz64(100), nz64(30))
        .expect("valid projected compaction limits");

    let control_directory = tempdir().expect("control temporary directory");
    let control_kernel =
        Kernel::open(control_directory.path().join("kernel.sqlite3")).expect("open control kernel");
    let control_session = create_session(&control_kernel);
    let control_model = Arc::new(ScriptedModel::new(
        four_operation_script(),
        Arc::new(Mutex::new(Vec::new())),
    ));
    let control_runtime = runtime_with_context(
        control_model,
        Arc::new(CompactingContextStrategy::new(
            limits,
            nz32(2),
            Arc::new(ThresholdSizer),
        )),
        "unprojected-multi-cut-v1",
    );
    drive_four_operations(&control_kernel, control_session, &control_runtime).await;
    assert_eq!(checkpoint_boundary(&control_kernel, control_session), 3);

    let projected_directory = tempdir().expect("projected temporary directory");
    let kernel = Kernel::open(projected_directory.path().join("kernel.sqlite3"))
        .expect("open projected kernel");
    let session_id = create_session(&kernel);
    let requests = Arc::new(Mutex::new(Vec::new()));
    let sized = Arc::new(Mutex::new(Vec::new()));
    let model = Arc::new(ScriptedModel::new(
        four_operation_script(),
        Arc::clone(&requests),
    ));
    let strategy = CompactingContextStrategy::with_projector(
        limits,
        nz32(2),
        Arc::new(RecordingSizer {
            requests: Arc::clone(&sized),
        }),
        Arc::new(PrefixProjector),
    );
    let runtime = runtime_with_context(model, Arc::new(strategy), "projected-compaction-v1");

    drive_four_operations(&kernel, session_id, &runtime).await;

    let requests = requests.lock().expect("request lock");
    assert_eq!(requests.len(), 5, "fourth operation must compact once");
    assert!(contains_projected_message(&requests[0]));
    assert!(contains_projected_message(&requests[1]));
    assert!(contains_projected_message(&requests[2]));
    assert!(!contains_projected_message(&requests[3]));
    assert!(contains_projected_message(&requests[4]));
    assert_eq!(checkpoint_boundary(&kernel, session_id), 5);

    let sized = sized.lock().expect("sized request lock");
    let normal_without_checkpoint = sized.iter().filter(|request| {
        request.system_prompt == "Test durable compaction."
            && !request_contains(request, "[CONTEXT CHECKPOINT]")
    });
    assert!(
        normal_without_checkpoint
            .clone()
            .all(contains_projected_message)
    );
    let retained_tail_lengths = normal_without_checkpoint
        .filter(|request| {
            request_contains(request, "Fourth question.") && request.messages.len() < 8
        })
        .map(|request| request.messages.len())
        .collect::<Vec<_>>();
    assert!(retained_tail_lengths.contains(&4));
    assert!(retained_tail_lengths.contains(&2));
}

struct PrefixProjector;

impl ContextProjector for PrefixProjector {
    fn project(&self, messages: Vec<Message>) -> Result<Vec<Message>, ContextStrategyError> {
        let mut projected = Vec::with_capacity(messages.len() + 1);
        projected.push(Message::user_text(PROJECTED_MESSAGE));
        projected.extend(messages);
        Ok(projected)
    }
}

struct RecordingSizer {
    requests: Arc<Mutex<Vec<ModelRequest>>>,
}

impl ContextSizer for RecordingSizer {
    fn estimate_input_tokens(&self, request: &ModelRequest) -> u64 {
        self.requests
            .lock()
            .expect("sized request lock")
            .push(request.clone());
        ThresholdSizer.estimate_input_tokens(request)
    }
}

fn contains_projected_message(request: &ModelRequest) -> bool {
    request
        .messages
        .iter()
        .any(|message| message == &Message::user_text(PROJECTED_MESSAGE))
}

fn request_contains(request: &ModelRequest, text: &str) -> bool {
    serde_json::to_string(request)
        .expect("encode inspected request")
        .contains(text)
}

fn four_operation_script() -> impl IntoIterator<Item = Script> {
    [
        Script::response(text_response("First answer.")),
        Script::response(text_response("Second answer.")),
        Script::response(text_response("Third answer.")),
        Script::response(text_response(super::compaction_support::SUMMARY)),
        Script::response(text_response("Fourth answer.")),
    ]
}

async fn drive_four_operations(
    kernel: &Kernel,
    session_id: renoa_kernel::SessionId,
    runtime: &renoa_kernel::Runtime,
) {
    submit_and_drive(kernel, session_id, runtime, "First question.").await;
    submit_and_drive(kernel, session_id, runtime, "Second question.").await;
    submit_and_drive(kernel, session_id, runtime, "Third question.").await;
    submit_and_drive(kernel, session_id, runtime, "Fourth question.").await;
}

fn checkpoint_boundary(kernel: &Kernel, session_id: renoa_kernel::SessionId) -> u64 {
    kernel
        .events_after(session_id, renoa_kernel::EventCursor::START)
        .expect("read checkpoint event")
        .events
        .into_iter()
        .find(|event| event.kind == renoa_agent_loop::CONTEXT_CHECKPOINT_EVENT_KIND)
        .and_then(|event| event.payload["covered_through_sequence"].as_u64())
        .expect("one checkpoint boundary")
}

fn nz64(value: u64) -> NonZeroU64 {
    NonZeroU64::new(value).expect("test value is non-zero")
}
