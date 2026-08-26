use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, Ordering},
};

use renoa_agent::{Message, ModelResponse, ToolSpec};
use renoa_agent_loop::{
    COMPACTION_RESULT_EVENT_KIND, CONTEXT_CHECKPOINT_EVENT_KIND, CompactionPlan,
    CompactionValidationError, ContextInput, ContextPreparation, ContextStrategy,
    ContextStrategyError, ExplicitCompactionPreparation,
};
use renoa_kernel::{DriveResult, EffectStatus, EventCursor, Kernel, OperationOutcome};
use tempfile::tempdir;

use super::compaction_support::{
    SUMMARY, Script, ScriptedModel, ThresholdSizer, compacting_strategy, create_session, nz32,
    runtime_with_context, submit, submit_and_drive, submit_compaction, text_response,
};

#[tokio::test]
async fn settled_summary_activates_after_restart_without_another_model_call() {
    let directory = tempdir().expect("temporary directory");
    let database = directory.path().join("kernel.sqlite3");
    let kernel = Arc::new(Kernel::open(&database).expect("open kernel"));
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
    let strategy = Arc::new(PanicOnceAfterSummarySettlement {
        inner: compacting_strategy(Arc::new(ThresholdSizer), nz32(2)),
        panicked: AtomicBool::new(false),
    });
    let runtime = Arc::new(runtime_with_context(
        model,
        strategy,
        "panic-once-after-summary-v1",
    ));

    submit_and_drive(&kernel, session_id, &runtime, "First question.").await;
    let second = submit(&kernel, session_id, "Second question.");
    let driver = Arc::clone(&kernel);
    let driven_runtime = Arc::clone(&runtime);
    let task = tokio::spawn(async move { driver.drive(session_id, &driven_runtime).await });
    assert!(task.await.expect_err("injected process loss").is_panic());

    let interrupted = kernel.inspect(session_id).expect("inspect settled summary");
    let summary_effect = interrupted.operations[1].effects[0].clone();
    assert_eq!(summary_effect.status, EffectStatus::Settled);
    assert_eq!(summary_effect.dispatch_count, 1);
    assert_eq!(requests.lock().expect("request lock").len(), 2);
    assert_eq!(checkpoint_count(&kernel, session_id), 0);
    drop(kernel);

    let reopened = Kernel::open(&database).expect("reopen kernel");
    assert_eq!(
        reopened
            .drive(session_id, &runtime)
            .await
            .expect("activate settled summary"),
        DriveResult::Finished {
            operation_id: second,
            outcome: OperationOutcome::Completed,
        }
    );
    let recovered = reopened.inspect(session_id).expect("inspect activation");
    assert_eq!(
        recovered.operations[1].effects[0].effect_id,
        summary_effect.effect_id
    );
    assert_eq!(recovered.operations[1].effects[0].dispatch_count, 1);
    assert_eq!(requests.lock().expect("request lock").len(), 3);
    assert_eq!(checkpoint_count(&reopened, session_id), 1);
}

#[tokio::test]
async fn settled_explicit_summary_finishes_after_restart_without_a_normal_model_call() {
    let directory = tempdir().expect("temporary directory");
    let database = directory.path().join("kernel.sqlite3");
    let kernel = Arc::new(Kernel::open(&database).expect("open kernel"));
    let session_id = create_session(&kernel);
    let requests = Arc::new(Mutex::new(Vec::new()));
    let model = Arc::new(ScriptedModel::new(
        [
            Script::response(text_response("First answer.")),
            Script::response(text_response(SUMMARY)),
        ],
        Arc::clone(&requests),
    ));
    let strategy = Arc::new(PanicOnceAfterSummarySettlement {
        inner: compacting_strategy(Arc::new(ThresholdSizer), nz32(2)),
        panicked: AtomicBool::new(false),
    });
    let runtime = Arc::new(runtime_with_context(
        model,
        strategy,
        "panic-once-after-explicit-summary-v1",
    ));

    submit_and_drive(&kernel, session_id, &runtime, "First question.").await;
    let compact = submit_compaction(&kernel, session_id);
    let driver = Arc::clone(&kernel);
    let driven_runtime = Arc::clone(&runtime);
    let task = tokio::spawn(async move { driver.drive(session_id, &driven_runtime).await });
    assert!(task.await.expect_err("injected process loss").is_panic());

    let interrupted = kernel.inspect(session_id).expect("inspect settled summary");
    let summary_effect = interrupted.operations[1].effects[0].clone();
    assert_eq!(summary_effect.status, EffectStatus::Settled);
    assert_eq!(summary_effect.dispatch_count, 1);
    assert_eq!(requests.lock().expect("request lock").len(), 2);
    assert_eq!(checkpoint_count(&kernel, session_id), 0);
    drop(kernel);

    let reopened = Kernel::open(&database).expect("reopen kernel");
    assert_eq!(
        reopened
            .drive(session_id, &runtime)
            .await
            .expect("activate settled explicit summary"),
        DriveResult::Finished {
            operation_id: compact,
            outcome: OperationOutcome::Completed,
        }
    );
    let recovered = reopened.inspect(session_id).expect("inspect activation");
    assert_eq!(
        recovered.operations[1].effects[0].effect_id,
        summary_effect.effect_id
    );
    assert_eq!(recovered.operations[1].effects[0].dispatch_count, 1);
    assert_eq!(requests.lock().expect("request lock").len(), 2);
    assert_eq!(checkpoint_count(&reopened, session_id), 1);
    assert_eq!(
        event_count(&reopened, session_id, COMPACTION_RESULT_EVENT_KIND),
        1
    );
}

struct PanicOnceAfterSummarySettlement {
    inner: renoa_agent_loop::CompactingContextStrategy,
    panicked: AtomicBool,
}

impl ContextStrategy for PanicOnceAfterSummarySettlement {
    fn project(&self, input: ContextInput) -> Result<Vec<Message>, ContextStrategyError> {
        self.inner.project(input)
    }

    fn prepare(&self, input: ContextInput) -> Result<ContextPreparation, ContextStrategyError> {
        self.inner.prepare(input)
    }

    fn prepare_explicit_compaction(
        &self,
        input: &ContextInput,
    ) -> Result<ExplicitCompactionPreparation, ContextStrategyError> {
        self.inner.prepare_explicit_compaction(input)
    }

    fn estimate_after_explicit_compaction(
        &self,
        input: &ContextInput,
        plan: &CompactionPlan,
        summary: &str,
    ) -> Result<u64, ContextStrategyError> {
        self.inner
            .estimate_after_explicit_compaction(input, plan, summary)
    }

    fn validate_compaction(
        &self,
        plan: &CompactionPlan,
        response: &ModelResponse,
        system_prompt: &str,
        tools: &[ToolSpec],
    ) -> Result<String, CompactionValidationError> {
        assert!(
            self.panicked.swap(true, Ordering::SeqCst),
            "injected process loss after summary settlement"
        );
        self.inner
            .validate_compaction(plan, response, system_prompt, tools)
    }
}

fn checkpoint_count(kernel: &Kernel, session_id: renoa_kernel::SessionId) -> usize {
    event_count(kernel, session_id, CONTEXT_CHECKPOINT_EVENT_KIND)
}

fn event_count(kernel: &Kernel, session_id: renoa_kernel::SessionId, kind: &str) -> usize {
    kernel
        .events_after(session_id, EventCursor::START)
        .expect("read checkpoint events")
        .events
        .iter()
        .filter(|event| event.kind == kind)
        .count()
}
