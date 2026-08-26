use std::sync::{Arc, Mutex};

use renoa_agent::{Message, ModelRequest};
use renoa_agent_loop::{
    COMPACTION_RESULT_EVENT_KIND, CONTEXT_CHECKPOINT_EVENT_KIND, ContextSizer, MESSAGE_EVENT_KIND,
};
use renoa_kernel::{CancellationId, DriveResult, EventCursor, Kernel, OperationOutcome};
use tempfile::tempdir;
use tokio::sync::Notify;

use super::compaction_support::{
    SUMMARY, Script, ScriptedModel, ThresholdSizer, compacting_runtime, create_session, nz32,
    submit_and_drive, submit_compaction, text_response,
};

#[tokio::test]
async fn explicit_compaction_is_a_durable_turn_without_a_normal_model_call() {
    let directory = tempdir().expect("temporary directory");
    let kernel = Kernel::open(directory.path().join("kernel.sqlite3")).expect("open kernel");
    let session_id = create_session(&kernel);
    let requests = Arc::new(Mutex::new(Vec::new()));
    let model = Arc::new(ScriptedModel::new(
        [
            Script::response(text_response("First answer.")),
            Script::response(text_response(SUMMARY)),
            Script::response(text_response("Continued after compaction.")),
        ],
        Arc::clone(&requests),
    ));
    let runtime = compacting_runtime(model, Arc::new(ThresholdSizer), nz32(2));

    submit_and_drive(&kernel, session_id, &runtime, "First question.").await;
    let compact = submit_compaction(&kernel, session_id);
    assert_eq!(
        kernel
            .drive(session_id, &runtime)
            .await
            .expect("drive explicit compaction"),
        DriveResult::Finished {
            operation_id: compact,
            outcome: OperationOutcome::Completed,
        }
    );

    {
        let requests = requests.lock().expect("request lock");
        assert_eq!(requests.len(), 2, "compaction must end after its summary");
        let summary = &requests[1];
        assert!(summary.tools.is_empty());
        let encoded = serde_json::to_string(summary).expect("encode summary request");
        assert!(encoded.contains("First question."));
        assert!(encoded.contains("First answer."));
        assert!(!encoded.contains("/compact"));
    }

    let events = kernel
        .events_after(session_id, EventCursor::START)
        .expect("read durable journal")
        .events;
    assert_eq!(
        events
            .iter()
            .filter(|event| event.kind == MESSAGE_EVENT_KIND)
            .count(),
        2,
        "the control command must not become model-visible history"
    );
    let checkpoints = events
        .iter()
        .filter(|event| event.kind == CONTEXT_CHECKPOINT_EVENT_KIND)
        .collect::<Vec<_>>();
    assert_eq!(checkpoints.len(), 1);
    assert_eq!(checkpoints[0].payload["covered_through_sequence"], 1);
    let results = events
        .iter()
        .filter(|event| event.kind == COMPACTION_RESULT_EVENT_KIND)
        .collect::<Vec<_>>();
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].payload["estimated_input_tokens"], 10);

    submit_and_drive(&kernel, session_id, &runtime, "Continue.").await;
    let requests = requests.lock().expect("request lock");
    assert_eq!(requests.len(), 3);
    assert_eq!(requests[2].messages.len(), 2);
    assert!(matches!(&requests[2].messages[0], Message::User { .. }));
    let continued = serde_json::to_string(&requests[2]).expect("encode continued request");
    assert!(continued.contains("[CONTEXT CHECKPOINT]"));
    assert!(continued.contains("Continue."));
    assert!(!continued.contains("First question."));
}

#[tokio::test]
async fn explicit_compaction_of_an_up_to_date_checkpoint_uses_no_model() {
    let directory = tempdir().expect("temporary directory");
    let kernel = Kernel::open(directory.path().join("kernel.sqlite3")).expect("open kernel");
    let session_id = create_session(&kernel);
    let requests = Arc::new(Mutex::new(Vec::new()));
    let model = Arc::new(ScriptedModel::new(
        [
            Script::response(text_response("First answer.")),
            Script::response(text_response(SUMMARY)),
        ],
        Arc::clone(&requests),
    ));
    let runtime = compacting_runtime(model, Arc::new(ThresholdSizer), nz32(2));

    submit_and_drive(&kernel, session_id, &runtime, "First question.").await;
    for _ in 0..2 {
        let operation_id = submit_compaction(&kernel, session_id);
        assert_eq!(
            kernel
                .drive(session_id, &runtime)
                .await
                .expect("drive explicit compaction"),
            DriveResult::Finished {
                operation_id,
                outcome: OperationOutcome::Completed,
            }
        );
    }

    assert_eq!(requests.lock().expect("request lock").len(), 2);
    let events = kernel
        .events_after(session_id, EventCursor::START)
        .expect("read durable journal")
        .events;
    assert_eq!(
        events
            .iter()
            .filter(|event| event.kind == CONTEXT_CHECKPOINT_EVENT_KIND)
            .count(),
        1
    );
    assert_eq!(
        events
            .iter()
            .filter(|event| event.kind == COMPACTION_RESULT_EVENT_KIND)
            .count(),
        2,
        "each distinct control turn needs its own durable result"
    );
}

#[tokio::test]
async fn explicit_compaction_fails_before_sampling_when_no_summary_request_fits() {
    let directory = tempdir().expect("temporary directory");
    let kernel = Kernel::open(directory.path().join("kernel.sqlite3")).expect("open kernel");
    let session_id = create_session(&kernel);
    let requests = Arc::new(Mutex::new(Vec::new()));
    let model = Arc::new(ScriptedModel::new(
        [Script::response(text_response("First answer."))],
        Arc::clone(&requests),
    ));
    let runtime = compacting_runtime(model, Arc::new(OversizedSummarySizer), nz32(2));

    submit_and_drive(&kernel, session_id, &runtime, "First question.").await;
    let compact = submit_compaction(&kernel, session_id);
    assert_eq!(
        kernel
            .drive(session_id, &runtime)
            .await
            .expect("reject undispatchable explicit compaction"),
        DriveResult::Finished {
            operation_id: compact,
            outcome: OperationOutcome::Failed {
                reason: "context cannot be reduced below the provider limit: estimated 20 input tokens, dispatch limit 40".to_owned(),
            },
        }
    );
    assert_eq!(
        requests.lock().expect("request lock").len(),
        1,
        "an oversized summary request reached the model"
    );
    let events = kernel
        .events_after(session_id, EventCursor::START)
        .expect("read durable journal")
        .events;
    assert!(events.iter().all(|event| {
        event.kind != CONTEXT_CHECKPOINT_EVENT_KIND && event.kind != COMPACTION_RESULT_EVENT_KIND
    }));
}

struct OversizedSummarySizer;

impl ContextSizer for OversizedSummarySizer {
    fn estimate_input_tokens(&self, request: &ModelRequest) -> u64 {
        if request
            .system_prompt
            .starts_with("You create durable context checkpoints")
        {
            41
        } else {
            u64::try_from(request.messages.len()).expect("message count fits u64") * 10
        }
    }
}

#[tokio::test]
async fn cancelling_explicit_summary_never_activates_partial_compaction() {
    let directory = tempdir().expect("temporary directory");
    let kernel =
        Arc::new(Kernel::open(directory.path().join("kernel.sqlite3")).expect("open kernel"));
    let session_id = create_session(&kernel);
    let requests = Arc::new(Mutex::new(Vec::new()));
    let invoked = Arc::new(Notify::new());
    let model = Arc::new(ScriptedModel::new(
        [
            Script::response(text_response("First answer.")),
            Script::WaitForCancellation(Arc::clone(&invoked)),
        ],
        Arc::clone(&requests),
    ));
    let runtime = Arc::new(compacting_runtime(model, Arc::new(ThresholdSizer), nz32(2)));

    submit_and_drive(&kernel, session_id, &runtime, "First question.").await;
    let compact = submit_compaction(&kernel, session_id);
    let driver = Arc::clone(&kernel);
    let driven_runtime = Arc::clone(&runtime);
    let drive = tokio::spawn(async move { driver.drive(session_id, &driven_runtime).await });
    tokio::time::timeout(std::time::Duration::from_secs(1), invoked.notified())
        .await
        .expect("summary model was not invoked");
    kernel
        .request_cancellation(session_id, compact, CancellationId::new())
        .expect("request cancellation");

    assert_eq!(
        drive
            .await
            .expect("join driver")
            .expect("settle cancellation"),
        DriveResult::Finished {
            operation_id: compact,
            outcome: OperationOutcome::Cancelled,
        }
    );
    assert_eq!(requests.lock().expect("request lock").len(), 2);
    let events = kernel
        .events_after(session_id, EventCursor::START)
        .expect("read durable journal")
        .events;
    assert!(
        events.iter().all(|event| {
            event.kind != CONTEXT_CHECKPOINT_EVENT_KIND
                && event.kind != COMPACTION_RESULT_EVENT_KIND
        }),
        "cancelled summary activated partial compaction state"
    );
}
