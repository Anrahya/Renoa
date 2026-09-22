use std::sync::{Arc, Mutex, Weak};

use renoa_kernel::{
    AgentId, CancellationId, CancellationInput, CancellationTransition, Checkpoint, Command,
    CommandId, DriveResult, EffectAdapter, EffectBinding, EffectCompletion, EffectFact,
    EffectFuture, EffectInvocation, EffectOutcome, EffectRecovery, EffectRequest, EffectStatus,
    EventCursor, Kernel, KernelError, LoopBinding, LoopDecision, LoopError, LoopInput, LoopPlugin,
    NewEvent, OperationId, OperationOutcome, OperationStatus, Runtime, SessionId,
    UnknownEffectAbandonment, UnknownEffectInput,
};
use tempfile::tempdir;

#[tokio::test]
async fn exact_intent_and_dispatch_are_durable_before_adapter_invocation() {
    let directory = tempdir().expect("temporary directory");
    let kernel =
        Arc::new(Kernel::open(directory.path().join("kernel.sqlite3")).expect("open kernel"));
    let session_id = create_session(&kernel);
    let request = serde_json::json!({"path": "src/lib.rs"});
    kernel
        .submit(session_id, Command::new(CommandId::new(), request.clone()))
        .expect("submit command");
    let observed = Arc::new(Mutex::new(false));
    let adapter = Arc::new(ObservingAdapter {
        kernel: Arc::downgrade(&kernel),
        session_id,
        expected_request: request.clone(),
        observed: Arc::clone(&observed),
    });

    kernel
        .drive(
            session_id,
            &effect_runtime(EffectRecovery::NeverReplay, adapter, false),
        )
        .await
        .expect("drive operation");

    assert!(*observed.lock().expect("observation lock"));
    let snapshot = kernel.inspect(session_id).expect("inspect session");
    let effect = &snapshot.operations[0].effect_batches[0].effects[0];
    assert_eq!(effect.request, request);
    assert_eq!(effect.status, EffectStatus::Settled);
    assert_eq!(effect.dispatch_count, 1);
}

#[tokio::test]
async fn a_possibly_dispatched_safe_effect_replays_with_exact_identity() {
    let directory = tempdir().expect("temporary directory");
    let database = directory.path().join("kernel.sqlite3");
    let kernel = Arc::new(Kernel::open(&database).expect("open kernel"));
    let session_id = create_session(&kernel);
    kernel
        .submit(
            session_id,
            Command::new(
                CommandId::new(),
                serde_json::json!({"message": "deliver once"}),
            ),
        )
        .expect("submit command");
    let first_calls = Arc::new(Mutex::new(Vec::new()));
    let crashing = Arc::new(CrashingAdapter {
        calls: Arc::clone(&first_calls),
    });
    let crashing_runtime = effect_runtime(EffectRecovery::SafeToReplay, crashing, false);
    let expected_manifest = crashing_runtime.manifest().clone();
    let runner = Arc::clone(&kernel);
    let task = tokio::spawn(async move { runner.drive(session_id, &crashing_runtime).await });
    assert!(task.await.expect_err("adapter panic").is_panic());
    let first = first_calls
        .lock()
        .expect("first calls lock")
        .first()
        .cloned()
        .expect("first invocation");
    drop(kernel);

    let kernel = Kernel::open(&database).expect("reopen kernel");
    let replay_calls = Arc::new(Mutex::new(Vec::new()));
    let replay = Arc::new(RecordingAdapter {
        calls: Arc::clone(&replay_calls),
    });
    assert!(matches!(
        kernel
            .drive(
                session_id,
                &effect_runtime(EffectRecovery::SafeToReplay, replay, false),
            )
            .await
            .expect("recover safe effect"),
        DriveResult::Finished { .. }
    ));
    let replay_calls = replay_calls.lock().expect("replay calls lock");
    assert_eq!(replay_calls.len(), 1);
    assert_eq!(replay_calls[0].effect_id, first.effect_id);
    assert_eq!(replay_calls[0].request, first.request);
    assert_eq!(replay_calls[0].binding, "external");
    assert_eq!(replay_calls[0].binding_revision, "1");
    assert_eq!(replay_calls[0].runtime_manifest, expected_manifest);
    let snapshot = kernel
        .inspect(session_id)
        .expect("inspect recovered session");
    assert_eq!(
        snapshot.operations[0].effect_batches[0].effects[0].dispatch_count,
        2
    );
}

#[tokio::test]
async fn a_possibly_dispatched_unsafe_effect_becomes_unknown_without_replay() {
    let directory = tempdir().expect("temporary directory");
    let database = directory.path().join("kernel.sqlite3");
    let kernel = Arc::new(Kernel::open(&database).expect("open kernel"));
    let session_id = create_session(&kernel);
    let admission = kernel
        .submit(
            session_id,
            Command::new(
                CommandId::new(),
                serde_json::json!({"deploy": "production"}),
            ),
        )
        .expect("submit command");
    let crashing = Arc::new(CrashingAdapter {
        calls: Arc::new(Mutex::new(Vec::new())),
    });
    let runner = Arc::clone(&kernel);
    let task = tokio::spawn(async move {
        runner
            .drive(
                session_id,
                &effect_runtime(EffectRecovery::NeverReplay, crashing, false),
            )
            .await
    });
    assert!(task.await.expect_err("adapter panic").is_panic());
    drop(kernel);

    let kernel = Kernel::open(&database).expect("reopen kernel");
    let recovery_calls = Arc::new(Mutex::new(Vec::new()));
    let recovery = Arc::new(RecordingAdapter {
        calls: Arc::clone(&recovery_calls),
    });
    assert_eq!(
        kernel
            .drive(
                session_id,
                &effect_runtime(EffectRecovery::NeverReplay, recovery, false),
            )
            .await
            .expect("recover unsafe effect"),
        DriveResult::Blocked {
            operation_id: admission.operation_id,
        }
    );
    assert!(
        recovery_calls
            .lock()
            .expect("recovery calls lock")
            .is_empty()
    );
    let snapshot = kernel.inspect(session_id).expect("inspect blocked session");
    assert_eq!(
        snapshot.operations[0].status,
        OperationStatus::OutcomeUnknown
    );
    assert_eq!(
        snapshot.operations[0].effect_batches[0].effects[0].status,
        EffectStatus::OutcomeUnknown
    );
}

#[tokio::test]
async fn a_live_unknown_safe_effect_is_replayed_through_the_same_effect_row() {
    let directory = tempdir().expect("temporary directory");
    let database = directory.path().join("kernel.sqlite3");
    let kernel = Kernel::open(&database).expect("open kernel");
    let session_id = create_session(&kernel);
    let admission = kernel
        .submit(
            session_id,
            Command::new(
                CommandId::new(),
                serde_json::json!({"message": "replay once"}),
            ),
        )
        .expect("submit command");
    let calls = Arc::new(Mutex::new(Vec::new()));
    let runtime = effect_runtime(
        EffectRecovery::SafeToReplay,
        Arc::new(UncertainAdapter {
            unknown_reports: 1,
            calls: Arc::clone(&calls),
        }),
        false,
    );
    let expected_manifest = runtime.manifest().clone();

    assert!(matches!(
        kernel
            .drive(session_id, &runtime)
            .await
            .expect("drive live unknown effect"),
        DriveResult::Finished {
            operation_id,
            outcome: OperationOutcome::Completed,
        } if operation_id == admission.operation_id
    ));

    let calls = calls.lock().expect("calls lock");
    assert_eq!(calls.len(), 2, "one live unknown dispatch and one replay");
    assert_eq!(calls[0].effect_id, calls[1].effect_id);
    assert_eq!(calls[0].request, calls[1].request);
    assert_eq!(calls[0].binding, calls[1].binding);
    assert_eq!(calls[0].binding_revision, calls[1].binding_revision);
    assert_eq!(calls[0].runtime_manifest, expected_manifest);
    assert_eq!(calls[1].runtime_manifest, expected_manifest);
    drop(calls);

    let snapshot = kernel
        .inspect(session_id)
        .expect("inspect replayed session");
    assert_eq!(snapshot.operations[0].effect_batches[0].effects.len(), 1);
    let effect = &snapshot.operations[0].effect_batches[0].effects[0];
    assert_eq!(effect.status, EffectStatus::Settled);
    assert_eq!(effect.dispatch_count, 2);
    assert_eq!(
        effect.outcome,
        Some(EffectOutcome::Success(serde_json::json!({"result": "ok"})))
    );
}

#[tokio::test]
async fn a_second_live_unknown_safe_dispatch_becomes_durable_unknown_without_a_third() {
    let directory = tempdir().expect("temporary directory");
    let database = directory.path().join("kernel.sqlite3");
    let kernel = Kernel::open(&database).expect("open kernel");
    let session_id = create_session(&kernel);
    let admission = kernel
        .submit(
            session_id,
            Command::new(
                CommandId::new(),
                serde_json::json!({"message": "never settles"}),
            ),
        )
        .expect("submit command");
    let calls = Arc::new(Mutex::new(Vec::new()));
    let runtime = effect_runtime(
        EffectRecovery::SafeToReplay,
        Arc::new(UncertainAdapter {
            unknown_reports: usize::MAX,
            calls: Arc::clone(&calls),
        }),
        false,
    );

    assert_eq!(
        kernel
            .drive(session_id, &runtime)
            .await
            .expect("drive twice unknown effect"),
        DriveResult::Blocked {
            operation_id: admission.operation_id,
        }
    );
    assert_eq!(calls.lock().expect("calls lock").len(), 2);

    assert_eq!(
        kernel
            .drive(session_id, &runtime)
            .await
            .expect("redrive durable unknown effect"),
        DriveResult::Blocked {
            operation_id: admission.operation_id,
        }
    );
    assert_eq!(
        calls.lock().expect("calls lock").len(),
        2,
        "a durably unknown effect is never dispatched again"
    );

    let snapshot = kernel
        .inspect(session_id)
        .expect("inspect durable unknown session");
    assert_eq!(
        snapshot.operations[0].status,
        OperationStatus::OutcomeUnknown
    );
    let effect = &snapshot.operations[0].effect_batches[0].effects[0];
    assert_eq!(effect.status, EffectStatus::OutcomeUnknown);
    assert_eq!(effect.dispatch_count, 2);
    assert_eq!(effect.outcome, None);

    let outcome = kernel
        .abandon_unknown_effect(session_id, admission.operation_id, &runtime)
        .expect("abandon durable unknown effect");
    assert!(matches!(
        outcome,
        OperationOutcome::Failed { ref reason }
            if reason == "effect outcome is unknown; operation was abandoned"
    ));
}

#[tokio::test]
async fn a_live_unknown_never_replay_effect_is_never_redispatched() {
    let directory = tempdir().expect("temporary directory");
    let database = directory.path().join("kernel.sqlite3");
    let kernel = Kernel::open(&database).expect("open kernel");
    let session_id = create_session(&kernel);
    let admission = kernel
        .submit(
            session_id,
            Command::new(
                CommandId::new(),
                serde_json::json!({"deploy": "production"}),
            ),
        )
        .expect("submit command");
    let calls = Arc::new(Mutex::new(Vec::new()));
    let runtime = effect_runtime(
        EffectRecovery::NeverReplay,
        Arc::new(UncertainAdapter {
            unknown_reports: usize::MAX,
            calls: Arc::clone(&calls),
        }),
        false,
    );

    assert_eq!(
        kernel
            .drive(session_id, &runtime)
            .await
            .expect("drive unknown unsafe effect"),
        DriveResult::Blocked {
            operation_id: admission.operation_id,
        }
    );
    assert_eq!(
        calls.lock().expect("calls lock").len(),
        1,
        "an unsafe effect keeps its single dispatch"
    );
    let snapshot = kernel.inspect(session_id).expect("inspect unsafe session");
    assert_eq!(
        snapshot.operations[0].status,
        OperationStatus::OutcomeUnknown
    );
    assert_eq!(
        snapshot.operations[0].effect_batches[0].effects[0].status,
        EffectStatus::OutcomeUnknown
    );
    assert_eq!(
        snapshot.operations[0].effect_batches[0].effects[0].dispatch_count,
        1
    );
}

#[tokio::test]
async fn a_cancellation_requested_during_a_live_unknown_dispatch_prevents_the_replay() {
    let directory = tempdir().expect("temporary directory");
    let database = directory.path().join("kernel.sqlite3");
    let kernel = Arc::new(Kernel::open(&database).expect("open kernel"));
    let session_id = create_session(&kernel);
    let admission = kernel
        .submit(
            session_id,
            Command::new(
                CommandId::new(),
                serde_json::json!({"message": "cancel me"}),
            ),
        )
        .expect("submit command");
    let calls = Arc::new(Mutex::new(Vec::new()));
    let runtime = effect_runtime(
        EffectRecovery::SafeToReplay,
        Arc::new(CancellingUnknownAdapter {
            kernel: Arc::downgrade(&kernel),
            session_id,
            operation_id: admission.operation_id,
            calls: Arc::clone(&calls),
        }),
        false,
    );

    assert_eq!(
        kernel
            .drive(session_id, &runtime)
            .await
            .expect("drive cancelled unknown effect"),
        DriveResult::Finished {
            operation_id: admission.operation_id,
            outcome: OperationOutcome::Cancelled,
        }
    );
    assert_eq!(
        calls.lock().expect("calls lock").len(),
        1,
        "a durable cancellation prevents the second adapter dispatch"
    );
    let snapshot = kernel
        .inspect(session_id)
        .expect("inspect cancelled session");
    assert_eq!(snapshot.operations[0].status, OperationStatus::Cancelled);
    assert_eq!(
        snapshot.operations[0].effect_batches[0].effects[0].dispatch_count,
        1
    );
    assert_eq!(
        snapshot.operations[0].effect_batches[0].effects[0].status,
        EffectStatus::OutcomeUnknown
    );
}

#[tokio::test]
async fn settled_effect_and_next_loop_input_are_atomic_and_never_repeated() {
    let directory = tempdir().expect("temporary directory");
    let database = directory.path().join("kernel.sqlite3");
    let kernel = Kernel::open(&database).expect("open kernel");
    let session_id = create_session(&kernel);
    kernel
        .submit(
            session_id,
            Command::new(CommandId::new(), serde_json::json!({"read": "state"})),
        )
        .expect("submit command");
    let calls = Arc::new(Mutex::new(Vec::new()));
    let adapter = Arc::new(RecordingAdapter {
        calls: Arc::clone(&calls),
    });

    assert!(matches!(
        kernel
            .drive(
                session_id,
                &effect_runtime(EffectRecovery::SafeToReplay, adapter, true),
            )
            .await,
        Err(KernelError::Loop(error))
            if error.message() == "injected post-settlement failure"
    ));
    let settled = kernel.inspect(session_id).expect("inspect settlement");
    assert_eq!(settled.operations[0].status, OperationStatus::Running);
    assert_eq!(
        settled.operations[0].effect_batches[0].effects[0].status,
        EffectStatus::Settled
    );
    assert_eq!(
        settled.operations[0].effect_batches[0].effects[0].dispatch_count,
        1
    );
    assert!(
        kernel
            .events_after(session_id, EventCursor::START)
            .expect("read events")
            .events
            .is_empty()
    );
    drop(kernel);

    let kernel = Kernel::open(&database).expect("reopen kernel");
    let adapter = Arc::new(RecordingAdapter {
        calls: Arc::clone(&calls),
    });
    kernel
        .drive(
            session_id,
            &effect_runtime(EffectRecovery::SafeToReplay, adapter, false),
        )
        .await
        .expect("consume settled effect");
    assert_eq!(calls.lock().expect("calls lock").len(), 1);
    assert_eq!(
        kernel
            .events_after(session_id, EventCursor::START)
            .expect("read final event")
            .events
            .len(),
        1
    );
}

fn create_session(kernel: &Kernel) -> SessionId {
    let agent_id = AgentId::new();
    let session_id = SessionId::new();
    kernel.create_agent(agent_id).expect("create agent");
    kernel
        .create_session(session_id, agent_id)
        .expect("create session");
    session_id
}

fn effect_runtime(
    recovery: EffectRecovery,
    adapter: Arc<dyn EffectAdapter>,
    fail_after_effect: bool,
) -> Runtime {
    Runtime::new(
        LoopBinding::new(
            "effect-loop",
            "1",
            Arc::new(EffectLoop {
                recovery,
                fail_after_effect,
            }),
        ),
        1,
        "effect-config-1",
        vec![EffectBinding::new("external", "1", adapter)],
    )
    .expect("valid runtime")
}

struct EffectLoop {
    recovery: EffectRecovery,
    fail_after_effect: bool,
}

impl LoopPlugin for EffectLoop {
    fn decide(&self, input: LoopInput) -> Result<LoopDecision, LoopError> {
        match input.effect_batch {
            None => Ok(LoopDecision::InvokeEffects {
                checkpoint: Checkpoint::new(1, serde_json::json!({"step": "effect_requested"})),
                effects: vec![EffectRequest {
                    binding: "external".to_owned(),
                    request: input.command.content().clone(),
                    recovery: self.recovery,
                }],
            }),
            Some(_batch) if self.fail_after_effect => {
                Err(LoopError::new("injected post-settlement failure"))
            }
            Some(batch) => Ok(LoopDecision::Complete {
                checkpoint: Checkpoint::new(1, serde_json::json!({"step": "done"})),
                events: vec![NewEvent::new(
                    "effect_result",
                    serde_json::to_value(&batch.effects[0].outcome).expect("serialize outcome"),
                )],
            }),
        }
    }

    fn cancel_operation(
        &self,
        input: CancellationInput,
    ) -> Result<CancellationTransition, LoopError> {
        assert!(
            matches!(
                input
                    .effect_batch
                    .as_ref()
                    .and_then(|batch| batch.effects.first()),
                Some(EffectFact::OutcomeUnknown(_))
            ),
            "a dispatched effect must be classified as possibly run when cancellation closes it"
        );
        Ok(CancellationTransition {
            checkpoint: Checkpoint::new(1, serde_json::json!({"step": "cancelled"})),
            events: vec![NewEvent::new(
                "effect_cancelled",
                serde_json::json!({"cancelled": true}),
            )],
        })
    }

    fn abandon_unknown_effect(
        &self,
        _input: UnknownEffectInput,
    ) -> Result<UnknownEffectAbandonment, LoopError> {
        Ok(UnknownEffectAbandonment {
            checkpoint: Checkpoint::new(1, serde_json::json!({"step": "abandoned"})),
            events: vec![NewEvent::new(
                "effect_abandoned",
                serde_json::json!({"abandoned": true}),
            )],
        })
    }
}

struct RecordingAdapter {
    calls: Arc<Mutex<Vec<EffectInvocation>>>,
}

impl EffectAdapter for RecordingAdapter {
    fn invoke(&self, invocation: EffectInvocation) -> EffectFuture<'_> {
        self.calls.lock().expect("calls lock").push(invocation);
        Box::pin(std::future::ready(
            EffectOutcome::Success(serde_json::json!({"result": "ok"})).into(),
        ))
    }
}

struct CrashingAdapter {
    calls: Arc<Mutex<Vec<EffectInvocation>>>,
}

impl EffectAdapter for CrashingAdapter {
    fn invoke(&self, invocation: EffectInvocation) -> EffectFuture<'_> {
        self.calls.lock().expect("calls lock").push(invocation);
        panic!("injected process loss after dispatch")
    }
}

/// Reports an unknown outcome for the first `unknown_reports` invocations, then
/// settles successfully. Records every invocation in dispatch order.
struct UncertainAdapter {
    unknown_reports: usize,
    calls: Arc<Mutex<Vec<EffectInvocation>>>,
}

impl EffectAdapter for UncertainAdapter {
    fn invoke(&self, invocation: EffectInvocation) -> EffectFuture<'_> {
        let attempt = {
            let mut calls = self.calls.lock().expect("calls lock");
            calls.push(invocation);
            calls.len()
        };
        if attempt <= self.unknown_reports {
            Box::pin(std::future::ready(EffectCompletion::OutcomeUnknown))
        } else {
            Box::pin(std::future::ready(
                EffectOutcome::Success(serde_json::json!({"result": "ok"})).into(),
            ))
        }
    }
}

/// Durably requests cancellation before reporting an unknown outcome, so the
/// ordering between the request and the kernel's replay decision is exact.
struct CancellingUnknownAdapter {
    kernel: Weak<Kernel>,
    session_id: SessionId,
    operation_id: OperationId,
    calls: Arc<Mutex<Vec<EffectInvocation>>>,
}

impl EffectAdapter for CancellingUnknownAdapter {
    fn invoke(&self, invocation: EffectInvocation) -> EffectFuture<'_> {
        self.calls.lock().expect("calls lock").push(invocation);
        let kernel = self.kernel.upgrade().expect("kernel still live");
        kernel
            .request_cancellation(self.session_id, self.operation_id, CancellationId::new())
            .expect("request cancellation during the effect");
        Box::pin(std::future::ready(EffectCompletion::OutcomeUnknown))
    }
}

struct ObservingAdapter {
    kernel: Weak<Kernel>,
    session_id: SessionId,
    expected_request: serde_json::Value,
    observed: Arc<Mutex<bool>>,
}

impl EffectAdapter for ObservingAdapter {
    fn invoke(&self, _invocation: EffectInvocation) -> EffectFuture<'_> {
        let kernel = self.kernel.upgrade().expect("kernel still live");
        let snapshot = kernel
            .inspect(self.session_id)
            .expect("inspect during effect");
        let effect = &snapshot.operations[0].effect_batches[0].effects[0];
        assert_eq!(effect.status, EffectStatus::DispatchStarted);
        assert_eq!(effect.request, self.expected_request);
        assert_eq!(effect.dispatch_count, 1);
        *self.observed.lock().expect("observation lock") = true;
        Box::pin(std::future::ready(
            EffectOutcome::Success(serde_json::json!({"result": "ok"})).into(),
        ))
    }
}
