#[path = "durable_compaction/activation_recovery.rs"]
mod activation_recovery;
mod compaction_support;
#[path = "durable_compaction/repeated_active_turn.rs"]
mod repeated_active_turn;
#[path = "durable_compaction/replaceability.rs"]
mod replaceability;

use std::sync::{Arc, Mutex};

use compaction_support::{
    SUMMARY, Script, ScriptedModel, ThresholdSizer, UnderestimatingSizer, compacting_runtime,
    create_session, nz32, submit, submit_and_drive, text_response, tool_response,
};
use renoa_agent::{ContentBlock, Message};
use renoa_agent_loop::{CONTEXT_CHECKPOINT_EVENT_KIND, MESSAGE_EVENT_KIND};
use renoa_kernel::{
    CancellationId, DriveResult, EffectStatus, EventCursor, Kernel, OperationOutcome,
};
use tempfile::tempdir;
use tokio::sync::Notify;

#[tokio::test]
async fn oversized_context_is_summarized_activated_and_reused() {
    let directory = tempdir().expect("temporary directory");
    let kernel = Kernel::open(directory.path().join("kernel.sqlite3")).expect("open kernel");
    let session_id = create_session(&kernel);
    let requests = Arc::new(Mutex::new(Vec::new()));
    let model = Arc::new(ScriptedModel::new(
        [
            Script::response(text_response("First answer.")),
            Script::response(text_response(SUMMARY)),
            Script::response(text_response("Second answer.")),
        ],
        Arc::clone(&requests),
    ));
    let runtime = compacting_runtime(model, Arc::new(ThresholdSizer), nz32(2));

    submit_and_drive(&kernel, session_id, &runtime, "First question.").await;
    let second = submit_and_drive(&kernel, session_id, &runtime, "Second question.").await;

    let requests = requests.lock().expect("request lock");
    assert_eq!(requests.len(), 3);
    let summary_request = requests[1].clone();
    assert!(summary_request.tools.is_empty());
    let encoded_summary = serde_json::to_string(&summary_request).expect("encode summary request");
    assert!(encoded_summary.contains("First question."));
    assert!(encoded_summary.contains("First answer."));
    assert!(!encoded_summary.contains("Second question."));
    assert!(!encoded_summary.contains(&second.to_string()));

    let continued = requests[2].clone();
    assert_eq!(continued.messages.len(), 2);
    let Message::User { content } = &continued.messages[0] else {
        panic!("checkpoint projection must begin with a user message");
    };
    let [ContentBlock::Text { text }] = content.as_slice() else {
        panic!("checkpoint projection must contain one text block");
    };
    assert!(text.contains(SUMMARY));
    let continued_json = serde_json::to_string(&continued).expect("encode continued request");
    assert!(continued_json.contains("[CONTEXT CHECKPOINT]"));
    assert!(continued_json.contains("Second question."));
    assert!(!continued_json.contains("First question."));
    assert!(!continued_json.contains("First answer."));
    drop(requests);

    let events = kernel
        .events_after(session_id, EventCursor::START)
        .expect("read durable journal")
        .events;
    assert_eq!(
        events
            .iter()
            .filter(|event| event.kind == MESSAGE_EVENT_KIND)
            .count(),
        4,
        "compaction must not delete or replace semantic messages"
    );
    let checkpoints = events
        .iter()
        .filter(|event| event.kind == CONTEXT_CHECKPOINT_EVENT_KIND)
        .collect::<Vec<_>>();
    assert_eq!(checkpoints.len(), 1);
    assert_eq!(checkpoints[0].payload["covered_through_sequence"], 1);
    assert_eq!(checkpoints[0].payload["summary"], SUMMARY);

    let snapshot = kernel.inspect(session_id).expect("inspect session");
    assert_eq!(snapshot.operations[1].effects.len(), 2);
    assert_eq!(
        snapshot.operations[1].effects[0].request,
        serde_json::to_value(&summary_request).expect("encode persisted summary request")
    );
    assert_eq!(
        snapshot.operations[1].effects[1].request,
        serde_json::to_value(&continued).expect("encode persisted continued request")
    );
}

#[tokio::test]
async fn malformed_summaries_exhaust_the_bound_without_activating() {
    let directory = tempdir().expect("temporary directory");
    let kernel = Kernel::open(directory.path().join("kernel.sqlite3")).expect("open kernel");
    let session_id = create_session(&kernel);
    let requests = Arc::new(Mutex::new(Vec::new()));
    let model = Arc::new(ScriptedModel::new(
        [
            Script::response(text_response("First answer.")),
            Script::response(tool_response()),
            Script::response(text_response("not a checkpoint")),
        ],
        Arc::clone(&requests),
    ));
    let runtime = compacting_runtime(model, Arc::new(ThresholdSizer), nz32(2));

    submit_and_drive(&kernel, session_id, &runtime, "First question.").await;
    let operation_id = submit(&kernel, session_id, "Second question.");
    let result = kernel
        .drive(session_id, &runtime)
        .await
        .expect("drive bounded compaction failure");

    assert!(matches!(
        result,
        DriveResult::Finished {
            operation_id: finished,
            outcome: OperationOutcome::Failed { ref reason },
        } if finished == operation_id && reason.contains("compaction response expected")
    ));
    assert_eq!(requests.lock().expect("request lock").len(), 3);
    let events = kernel
        .events_after(session_id, EventCursor::START)
        .expect("read durable journal")
        .events;
    assert!(
        events
            .iter()
            .all(|event| event.kind != CONTEXT_CHECKPOINT_EVENT_KIND)
    );
    assert_eq!(
        events
            .iter()
            .filter(|event| event.kind == MESSAGE_EVENT_KIND)
            .count(),
        3
    );
    let snapshot = kernel
        .inspect(session_id)
        .expect("inspect failed operation");
    assert_eq!(snapshot.operations[1].effects.len(), 2);
}

#[tokio::test]
async fn provider_overflow_forces_compaction_without_repeating_the_rejected_request() {
    let directory = tempdir().expect("temporary directory");
    let kernel = Kernel::open(directory.path().join("kernel.sqlite3")).expect("open kernel");
    let session_id = create_session(&kernel);
    let requests = Arc::new(Mutex::new(Vec::new()));
    let model = Arc::new(ScriptedModel::new(
        [
            Script::response(text_response("First answer.")),
            Script::ContextOverflow,
            Script::response(text_response(SUMMARY)),
            Script::response(text_response("Second answer.")),
        ],
        Arc::clone(&requests),
    ));
    let runtime = compacting_runtime(model, Arc::new(UnderestimatingSizer), nz32(2));

    submit_and_drive(&kernel, session_id, &runtime, "First question.").await;
    submit_and_drive(&kernel, session_id, &runtime, "Second question.").await;

    let requests = requests.lock().expect("request lock");
    assert_eq!(requests.len(), 4);
    assert_ne!(requests[1], requests[3]);
    let rejected = serde_json::to_string(&requests[1]).expect("encode rejected request");
    let recovered = serde_json::to_string(&requests[3]).expect("encode recovered request");
    assert!(rejected.contains("First question."));
    assert!(recovered.contains("[CONTEXT CHECKPOINT]"));
    assert!(!recovered.contains("First question."));
}

#[tokio::test]
async fn unknown_summary_outcome_blocks_without_activating_or_inventing_a_summary() {
    let directory = tempdir().expect("temporary directory");
    let kernel = Kernel::open(directory.path().join("kernel.sqlite3")).expect("open kernel");
    let session_id = create_session(&kernel);
    let requests = Arc::new(Mutex::new(Vec::new()));
    let model = Arc::new(ScriptedModel::new(
        [
            Script::response(text_response("First answer.")),
            Script::OutcomeUnknown,
        ],
        Arc::clone(&requests),
    ));
    let runtime = compacting_runtime(model, Arc::new(ThresholdSizer), nz32(2));

    submit_and_drive(&kernel, session_id, &runtime, "First question.").await;
    let second = submit(&kernel, session_id, "Second question.");
    assert_eq!(
        kernel
            .drive(session_id, &runtime)
            .await
            .expect("drive uncertain summary"),
        DriveResult::Blocked {
            operation_id: second,
        }
    );
    let blocked = kernel.inspect(session_id).expect("inspect blocked summary");
    assert_eq!(
        blocked.operations[1].effects[0].status,
        EffectStatus::OutcomeUnknown
    );
    assert_eq!(checkpoint_count(&kernel, session_id), 0);

    assert!(matches!(
        kernel
            .abandon_unknown_effect(session_id, second, &runtime)
            .expect("abandon unknown summary"),
        OperationOutcome::Failed { .. }
    ));
    assert_eq!(checkpoint_count(&kernel, session_id), 0);
    assert_eq!(
        kernel
            .events_after(session_id, EventCursor::START)
            .expect("read durable messages")
            .events
            .iter()
            .filter(|event| event.kind == MESSAGE_EVENT_KIND)
            .count(),
        3
    );
}

#[tokio::test]
async fn cancellation_during_summary_sampling_never_activates_the_checkpoint() {
    let directory = tempdir().expect("temporary directory");
    let kernel =
        Arc::new(Kernel::open(directory.path().join("kernel.sqlite3")).expect("open kernel"));
    let session_id = create_session(&kernel);
    let invoked = Arc::new(Notify::new());
    let requests = Arc::new(Mutex::new(Vec::new()));
    let model = Arc::new(ScriptedModel::new(
        [
            Script::response(text_response("First answer.")),
            Script::WaitForCancellation(Arc::clone(&invoked)),
        ],
        Arc::clone(&requests),
    ));
    let runtime = Arc::new(compacting_runtime(model, Arc::new(ThresholdSizer), nz32(2)));

    submit_and_drive(&kernel, session_id, &runtime, "First question.").await;
    let second = submit(&kernel, session_id, "Second question.");
    let driver = Arc::clone(&kernel);
    let driven_runtime = Arc::clone(&runtime);
    let drive = tokio::spawn(async move { driver.drive(session_id, &driven_runtime).await });
    invoked.notified().await;

    kernel
        .request_cancellation(session_id, second, CancellationId::new())
        .expect("request compaction cancellation");
    assert_eq!(
        drive
            .await
            .expect("join drive")
            .expect("settle cancellation"),
        DriveResult::Finished {
            operation_id: second,
            outcome: OperationOutcome::Cancelled,
        }
    );
    assert_eq!(checkpoint_count(&kernel, session_id), 0);
    assert_eq!(
        kernel
            .events_after(session_id, EventCursor::START)
            .expect("read durable messages")
            .events
            .iter()
            .filter(|event| event.kind == MESSAGE_EVENT_KIND)
            .count(),
        3
    );
}

#[tokio::test]
async fn interrupted_summary_replays_the_exact_intent_then_activates_once() {
    let directory = tempdir().expect("temporary directory");
    let database = directory.path().join("kernel.sqlite3");
    let kernel = Arc::new(Kernel::open(&database).expect("open kernel"));
    let session_id = create_session(&kernel);
    let requests = Arc::new(Mutex::new(Vec::new()));
    let model = Arc::new(ScriptedModel::new(
        [
            Script::response(text_response("First answer.")),
            Script::Panic,
            Script::response(text_response(SUMMARY)),
            Script::response(text_response("Second answer.")),
        ],
        Arc::clone(&requests),
    ));
    let runtime = Arc::new(compacting_runtime(model, Arc::new(ThresholdSizer), nz32(2)));

    submit_and_drive(&kernel, session_id, &runtime, "First question.").await;
    let second = submit(&kernel, session_id, "Second question.");
    let driver = Arc::clone(&kernel);
    let driven_runtime = Arc::clone(&runtime);
    let task = tokio::spawn(async move { driver.drive(session_id, &driven_runtime).await });
    assert!(task.await.expect_err("injected process loss").is_panic());

    let interrupted = kernel
        .inspect(session_id)
        .expect("inspect interrupted summary");
    let original = interrupted.operations[1].effects[0].clone();
    assert_eq!(original.status, EffectStatus::DispatchStarted);
    assert_eq!(original.dispatch_count, 1);
    assert!(
        kernel
            .events_after(session_id, EventCursor::START)
            .expect("read interrupted journal")
            .events
            .iter()
            .all(|event| event.kind != CONTEXT_CHECKPOINT_EVENT_KIND)
    );
    drop(kernel);

    let reopened = Kernel::open(&database).expect("reopen kernel");
    assert_eq!(
        reopened
            .drive(session_id, &runtime)
            .await
            .expect("resume compaction"),
        DriveResult::Finished {
            operation_id: second,
            outcome: OperationOutcome::Completed,
        }
    );
    let recovered = reopened
        .inspect(session_id)
        .expect("inspect recovered summary");
    let replayed = &recovered.operations[1].effects[0];
    assert_eq!(replayed.effect_id, original.effect_id);
    assert_eq!(replayed.request, original.request);
    assert_eq!(replayed.dispatch_count, 2);
    assert_eq!(replayed.status, EffectStatus::Settled);
    assert_eq!(
        reopened
            .events_after(session_id, EventCursor::START)
            .expect("read recovered journal")
            .events
            .iter()
            .filter(|event| event.kind == CONTEXT_CHECKPOINT_EVENT_KIND)
            .count(),
        1
    );
    let requests = requests.lock().expect("request lock");
    assert_eq!(requests[1], requests[2]);
}

fn checkpoint_count(kernel: &Kernel, session_id: renoa_kernel::SessionId) -> usize {
    kernel
        .events_after(session_id, EventCursor::START)
        .expect("read checkpoint events")
        .events
        .iter()
        .filter(|event| event.kind == CONTEXT_CHECKPOINT_EVENT_KIND)
        .count()
}
