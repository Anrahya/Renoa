use std::sync::{
    Arc, Mutex,
    atomic::{AtomicUsize, Ordering},
};

use tempfile::tempdir;

use crate::{
    AgentId, CancellationEffect, CancellationId, CancellationInput, CancellationTransition,
    Checkpoint, Command, CommandId, CrashPoint, DriveResult, EffectAdapter, EffectBinding,
    EffectCompletion, EffectFuture, EffectInvocation, EffectOutcome, EffectRecovery, EffectStatus,
    EventCursor, Kernel, LoopBinding, LoopDecision, LoopError, LoopInput, LoopPlugin, NewEvent,
    OperationOutcome, OperationStatus, Runtime, SessionId, UnknownEffectAbandonment,
    UnknownEffectInput,
};

#[tokio::test]
async fn never_replay_intent_committed_before_dispatch_runs_after_restart() {
    let directory = tempdir().expect("temporary directory");
    let database = directory.path().join("kernel.sqlite3");
    let (mut kernel, session_id) = kernel_with_command(&database);
    kernel.crash_at(CrashPoint::EffectIntentCommitted);
    let runtime = effect_runtime(EffectRecovery::NeverReplay, Arc::new(NeverCalledAdapter));
    let task = tokio::spawn(async move { kernel.drive(session_id, &runtime).await });
    assert!(task.await.expect_err("injected crash").is_panic());

    let kernel = Kernel::open(&database).expect("reopen kernel");
    let calls = Arc::new(Mutex::new(Vec::new()));
    kernel
        .drive(
            session_id,
            &effect_runtime(
                EffectRecovery::NeverReplay,
                Arc::new(RecordingAdapter(Arc::clone(&calls))),
            ),
        )
        .await
        .expect("dispatch committed intent");
    assert_eq!(calls.lock().expect("calls lock").len(), 1);
    let snapshot = kernel.inspect(session_id).expect("inspect session");
    assert_eq!(snapshot.operations[0].effects[0].dispatch_count, 1);
    assert_eq!(
        snapshot.operations[0].effects[0].status,
        EffectStatus::Settled
    );
}

#[tokio::test]
async fn dispatch_marker_makes_never_replay_recovery_conservatively_unknown() {
    let directory = tempdir().expect("temporary directory");
    let database = directory.path().join("kernel.sqlite3");
    let (mut kernel, session_id) = kernel_with_command(&database);
    kernel.crash_at(CrashPoint::EffectDispatchCommitted);
    let runtime = effect_runtime(EffectRecovery::NeverReplay, Arc::new(NeverCalledAdapter));
    let task = tokio::spawn(async move { kernel.drive(session_id, &runtime).await });
    assert!(task.await.expect_err("injected crash").is_panic());

    let kernel = Kernel::open(&database).expect("reopen kernel");
    let calls = Arc::new(Mutex::new(Vec::new()));
    assert!(matches!(
        kernel
            .drive(
                session_id,
                &effect_runtime(
                    EffectRecovery::NeverReplay,
                    Arc::new(RecordingAdapter(Arc::clone(&calls))),
                ),
            )
            .await
            .expect("recover dispatch marker"),
        DriveResult::Blocked { .. }
    ));
    assert!(calls.lock().expect("calls lock").is_empty());
    let snapshot = kernel.inspect(session_id).expect("inspect session");
    assert_eq!(
        snapshot.operations[0].effects[0].status,
        EffectStatus::OutcomeUnknown
    );
}

#[tokio::test]
async fn cancellation_of_committed_intent_proves_the_effect_never_dispatched() {
    let directory = tempdir().expect("temporary directory");
    let database = directory.path().join("cancel-intent.sqlite3");
    let (mut kernel, session_id) = kernel_with_command(&database);
    kernel.crash_at(CrashPoint::EffectIntentCommitted);
    let runtime = effect_runtime(EffectRecovery::NeverReplay, Arc::new(NeverCalledAdapter));
    let task = tokio::spawn(async move { kernel.drive(session_id, &runtime).await });
    assert!(task.await.expect_err("injected crash").is_panic());

    let kernel = Kernel::open(&database).expect("reopen kernel");
    let operation_id = kernel
        .inspect(session_id)
        .expect("inspect intent")
        .operations[0]
        .operation_id;
    kernel
        .request_cancellation(session_id, operation_id, CancellationId::new())
        .expect("request cancellation");
    assert_eq!(
        kernel
            .drive(
                session_id,
                &effect_runtime(EffectRecovery::NeverReplay, Arc::new(NeverCalledAdapter)),
            )
            .await
            .expect("close cancellation"),
        DriveResult::Finished {
            operation_id,
            outcome: OperationOutcome::Cancelled,
        }
    );
    let snapshot = kernel.inspect(session_id).expect("inspect cancellation");
    assert_eq!(snapshot.operations[0].status, OperationStatus::Cancelled);
    assert_eq!(
        snapshot.operations[0].effects[0].status,
        EffectStatus::IntentCommitted
    );
    assert_eq!(snapshot.operations[0].effects[0].dispatch_count, 0);
}

#[tokio::test]
async fn cancellation_after_a_dispatch_crash_never_replays_the_effect() {
    let directory = tempdir().expect("temporary directory");
    let database = directory.path().join("cancel-dispatch.sqlite3");
    let (mut kernel, session_id) = kernel_with_command(&database);
    kernel.crash_at(CrashPoint::EffectDispatchCommitted);
    let runtime = effect_runtime(EffectRecovery::SafeToReplay, Arc::new(NeverCalledAdapter));
    let task = tokio::spawn(async move { kernel.drive(session_id, &runtime).await });
    assert!(task.await.expect_err("injected crash").is_panic());

    let kernel = Kernel::open(&database).expect("reopen kernel");
    let operation_id = kernel
        .inspect(session_id)
        .expect("inspect dispatch")
        .operations[0]
        .operation_id;
    kernel
        .request_cancellation(session_id, operation_id, CancellationId::new())
        .expect("request cancellation");
    kernel
        .drive(
            session_id,
            &effect_runtime(EffectRecovery::SafeToReplay, Arc::new(NeverCalledAdapter)),
        )
        .await
        .expect("close cancellation without replay");
    let snapshot = kernel.inspect(session_id).expect("inspect cancellation");
    assert_eq!(snapshot.operations[0].status, OperationStatus::Cancelled);
    assert_eq!(
        snapshot.operations[0].effects[0].status,
        EffectStatus::OutcomeUnknown
    );
    assert_eq!(snapshot.operations[0].effects[0].dispatch_count, 1);
}

#[tokio::test]
async fn completed_before_settlement_safe_effect_replays_exactly() {
    let directory = tempdir().expect("temporary directory");
    let database = directory.path().join("kernel.sqlite3");
    let (mut kernel, session_id) = kernel_with_command(&database);
    let first_calls = Arc::new(Mutex::new(Vec::new()));
    kernel.crash_at(CrashPoint::EffectCompletedBeforeSettlement);
    let runtime = effect_runtime(
        EffectRecovery::SafeToReplay,
        Arc::new(RecordingAdapter(Arc::clone(&first_calls))),
    );
    let task = tokio::spawn(async move { kernel.drive(session_id, &runtime).await });
    assert!(task.await.expect_err("injected crash").is_panic());
    let first = first_calls.lock().expect("calls lock")[0].clone();

    let kernel = Kernel::open(&database).expect("reopen kernel");
    let replay_calls = Arc::new(Mutex::new(Vec::new()));
    kernel
        .drive(
            session_id,
            &effect_runtime(
                EffectRecovery::SafeToReplay,
                Arc::new(RecordingAdapter(Arc::clone(&replay_calls))),
            ),
        )
        .await
        .expect("recover completed effect");
    let replay_calls = replay_calls.lock().expect("calls lock");
    assert_eq!(replay_calls.len(), 1);
    assert_eq!(replay_calls[0].effect_id, first.effect_id);
    assert_eq!(replay_calls[0].request, first.request);
}

#[tokio::test]
async fn settlement_commit_survives_without_repeating_the_effect() {
    let directory = tempdir().expect("temporary directory");
    let database = directory.path().join("kernel.sqlite3");
    let (mut kernel, session_id) = kernel_with_command(&database);
    let calls = Arc::new(Mutex::new(Vec::new()));
    kernel.crash_at(CrashPoint::EffectSettlementCommitted);
    let runtime = effect_runtime(
        EffectRecovery::SafeToReplay,
        Arc::new(RecordingAdapter(Arc::clone(&calls))),
    );
    let task = tokio::spawn(async move { kernel.drive(session_id, &runtime).await });
    assert!(task.await.expect_err("injected crash").is_panic());

    let kernel = Kernel::open(&database).expect("reopen kernel");
    let snapshot = kernel.inspect(session_id).expect("inspect settlement");
    assert_eq!(
        snapshot.operations[0].effects[0].status,
        EffectStatus::Settled
    );
    assert!(
        kernel
            .events_after(session_id, EventCursor::START)
            .expect("read events")
            .events
            .is_empty()
    );
    kernel
        .drive(
            session_id,
            &effect_runtime(EffectRecovery::SafeToReplay, Arc::new(NeverCalledAdapter)),
        )
        .await
        .expect("consume settled result");
    assert_eq!(calls.lock().expect("calls lock").len(), 1);
}

#[tokio::test]
async fn abandonment_commit_survives_a_lost_reply_without_duplicate_events() {
    let directory = tempdir().expect("temporary directory");
    let database = directory.path().join("abandonment.sqlite3");
    let (mut kernel, session_id) = kernel_with_command(&database);
    let runtime = effect_runtime(EffectRecovery::NeverReplay, Arc::new(UnknownAdapter));
    let blocked = kernel
        .drive(session_id, &runtime)
        .await
        .expect("drive unknown effect");
    let DriveResult::Blocked { operation_id } = blocked else {
        panic!("unknown effect must block")
    };
    kernel.crash_at(CrashPoint::UnknownEffectAbandonmentCommitted);
    let lost_reply = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        kernel.abandon_unknown_effect(session_id, operation_id, &runtime)
    }));
    assert!(lost_reply.is_err());
    drop(kernel);

    let kernel = Kernel::open(&database).expect("reopen kernel");
    let outcome = kernel
        .abandon_unknown_effect(session_id, operation_id, &runtime)
        .expect("recover committed abandonment");
    assert!(matches!(
        outcome,
        OperationOutcome::Failed { ref reason }
            if reason == "effect outcome is unknown; operation was abandoned without replay"
    ));
    assert_eq!(
        kernel
            .events_after(session_id, EventCursor::START)
            .expect("read abandonment event")
            .events
            .len(),
        1
    );
}

#[tokio::test]
async fn cancellation_commit_survives_a_lost_reply_without_duplicate_events() {
    let directory = tempdir().expect("temporary directory");
    let database = directory.path().join("cancel-commit.sqlite3");
    let (mut kernel, session_id) = kernel_with_command(&database);
    kernel.crash_at(CrashPoint::EffectIntentCommitted);
    let runtime = effect_runtime(EffectRecovery::NeverReplay, Arc::new(NeverCalledAdapter));
    let task = tokio::spawn(async move { kernel.drive(session_id, &runtime).await });
    assert!(task.await.expect_err("intent crash").is_panic());

    let mut kernel = Kernel::open(&database).expect("reopen kernel");
    let operation_id = kernel
        .inspect(session_id)
        .expect("inspect intent")
        .operations[0]
        .operation_id;
    let cancellation_id = CancellationId::new();
    kernel
        .request_cancellation(session_id, operation_id, cancellation_id)
        .expect("request cancellation");
    kernel.crash_at(CrashPoint::CancellationCommitted);
    let runtime = effect_runtime(EffectRecovery::NeverReplay, Arc::new(NeverCalledAdapter));
    let task = tokio::spawn(async move { kernel.drive(session_id, &runtime).await });
    assert!(task.await.expect_err("lost cancellation reply").is_panic());

    let kernel = Kernel::open(&database).expect("reopen committed cancellation");
    kernel
        .request_cancellation(session_id, operation_id, cancellation_id)
        .expect("retry cancellation request");
    let snapshot = kernel.inspect(session_id).expect("inspect cancellation");
    assert_eq!(snapshot.operations[0].status, OperationStatus::Cancelled);
    assert_eq!(
        kernel
            .events_after(session_id, EventCursor::START)
            .expect("read cancellation event")
            .events
            .len(),
        1
    );
}

#[tokio::test]
async fn activation_and_terminal_commits_are_recovered_without_duplicate_loop_work() {
    let directory = tempdir().expect("temporary directory");
    let activation_database = directory.path().join("activation.sqlite3");
    let (mut kernel, session_id) = kernel_with_command(&activation_database);
    let calls = Arc::new(AtomicUsize::new(0));
    kernel.crash_at(CrashPoint::ActivationCommitted);
    let runtime = completing_runtime(Arc::clone(&calls));
    let task = tokio::spawn(async move { kernel.drive(session_id, &runtime).await });
    assert!(task.await.expect_err("activation crash").is_panic());
    assert_eq!(calls.load(Ordering::SeqCst), 0);
    let kernel = Kernel::open(&activation_database).expect("reopen activation kernel");
    kernel
        .drive(session_id, &completing_runtime(Arc::clone(&calls)))
        .await
        .expect("recover activation");
    assert_eq!(calls.load(Ordering::SeqCst), 1);

    let terminal_database = directory.path().join("terminal.sqlite3");
    let (mut kernel, session_id) = kernel_with_command(&terminal_database);
    kernel.crash_at(CrashPoint::TerminalCommitted);
    let runtime = completing_runtime(Arc::new(AtomicUsize::new(0)));
    let task = tokio::spawn(async move { kernel.drive(session_id, &runtime).await });
    assert!(task.await.expect_err("terminal crash").is_panic());
    let kernel = Kernel::open(&terminal_database).expect("reopen terminal kernel");
    assert_eq!(
        kernel
            .drive(
                session_id,
                &completing_runtime(Arc::new(AtomicUsize::new(0))),
            )
            .await
            .expect("settled operation is idle"),
        DriveResult::Idle
    );
    assert_eq!(
        kernel
            .events_after(session_id, EventCursor::START)
            .expect("read event")
            .events
            .len(),
        1
    );
}

fn kernel_with_command(path: &std::path::Path) -> (Kernel, SessionId) {
    let kernel = Kernel::open(path).expect("open kernel");
    let agent_id = AgentId::new();
    let session_id = SessionId::new();
    kernel.create_agent(agent_id).expect("create agent");
    kernel
        .create_session(session_id, agent_id)
        .expect("create session");
    kernel
        .submit(
            session_id,
            Command::new(CommandId::new(), serde_json::json!({"work": "exact"})),
        )
        .expect("submit command");
    (kernel, session_id)
}

fn effect_runtime(recovery: EffectRecovery, adapter: Arc<dyn EffectAdapter>) -> Runtime {
    Runtime::new(
        LoopBinding::new("effect-loop", "1", Arc::new(EffectLoop(recovery))),
        1,
        "effect-config-1",
        vec![EffectBinding::new("external", "1", adapter)],
    )
    .expect("valid runtime")
}

struct EffectLoop(EffectRecovery);

impl LoopPlugin for EffectLoop {
    fn decide(&self, input: LoopInput) -> Result<LoopDecision, LoopError> {
        if let Some(effect) = input.effect {
            Ok(LoopDecision::Complete {
                checkpoint: Checkpoint::new(1, serde_json::json!({"done": true})),
                events: vec![crate::NewEvent::new(
                    "result",
                    serde_json::to_value(effect.outcome).expect("serialize outcome"),
                )],
            })
        } else {
            Ok(LoopDecision::InvokeEffect {
                checkpoint: Checkpoint::new(1, serde_json::json!({"requested": true})),
                binding: "external".to_owned(),
                request: input.command.content().clone(),
                recovery: self.0,
            })
        }
    }

    fn abandon_unknown_effect(
        &self,
        input: UnknownEffectInput,
    ) -> Result<UnknownEffectAbandonment, LoopError> {
        Ok(UnknownEffectAbandonment {
            checkpoint: Checkpoint::new(1, serde_json::json!({"abandoned": true})),
            events: vec![NewEvent::new(
                "abandoned",
                serde_json::json!({"effect_id": input.effect.effect_id}),
            )],
        })
    }

    fn cancel_operation(
        &self,
        input: CancellationInput,
    ) -> Result<CancellationTransition, LoopError> {
        let effect_state = match input.effect {
            Some(CancellationEffect::NotDispatched(_)) => "not_dispatched",
            Some(CancellationEffect::Settled(_)) => "settled",
            Some(CancellationEffect::OutcomeUnknown(_)) => "outcome_unknown",
            None => "none",
        };
        Ok(CancellationTransition {
            checkpoint: Checkpoint::new(1, serde_json::json!({"cancelled": true})),
            events: vec![NewEvent::new(
                "cancelled",
                serde_json::json!({"effect": effect_state}),
            )],
        })
    }
}

struct RecordingAdapter(Arc<Mutex<Vec<EffectInvocation>>>);

impl EffectAdapter for RecordingAdapter {
    fn invoke(&self, invocation: EffectInvocation) -> EffectFuture<'_> {
        self.0.lock().expect("calls lock").push(invocation);
        Box::pin(std::future::ready(
            EffectOutcome::Success(serde_json::json!({"ok": true})).into(),
        ))
    }
}

struct NeverCalledAdapter;

impl EffectAdapter for NeverCalledAdapter {
    fn invoke(&self, _invocation: EffectInvocation) -> EffectFuture<'_> {
        panic!("adapter must not be invoked")
    }
}

struct UnknownAdapter;

impl EffectAdapter for UnknownAdapter {
    fn invoke(&self, _invocation: EffectInvocation) -> EffectFuture<'_> {
        Box::pin(std::future::ready(EffectCompletion::OutcomeUnknown))
    }
}

fn completing_runtime(calls: Arc<AtomicUsize>) -> Runtime {
    Runtime::new(
        LoopBinding::new("complete-loop", "1", Arc::new(CompletingLoop(calls))),
        1,
        "complete-config-1",
        Vec::new(),
    )
    .expect("valid runtime")
}

struct CompletingLoop(Arc<AtomicUsize>);

impl LoopPlugin for CompletingLoop {
    fn decide(&self, _input: LoopInput) -> Result<LoopDecision, LoopError> {
        self.0.fetch_add(1, Ordering::SeqCst);
        Ok(LoopDecision::Complete {
            checkpoint: Checkpoint::new(1, serde_json::json!({"done": true})),
            events: vec![crate::NewEvent::new("done", serde_json::json!(true))],
        })
    }
}
