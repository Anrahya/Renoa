use std::sync::{Arc, Mutex, Weak};

use renoa_kernel::{
    AgentId, Checkpoint, Command, CommandId, DriveResult, EffectAdapter, EffectBinding,
    EffectFuture, EffectInvocation, EffectOutcome, EffectRecovery, EffectStatus, EventCursor,
    Kernel, KernelError, LoopBinding, LoopDecision, LoopError, LoopInput, LoopPlugin, NewEvent,
    OperationStatus, Runtime, SessionId,
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
    let effect = &snapshot.operations[0].effects[0];
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
    assert_eq!(snapshot.operations[0].effects[0].dispatch_count, 2);
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
        snapshot.operations[0].effects[0].status,
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
        settled.operations[0].effects[0].status,
        EffectStatus::Settled
    );
    assert_eq!(settled.operations[0].effects[0].dispatch_count, 1);
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
        match input.effect {
            None => Ok(LoopDecision::InvokeEffect {
                checkpoint: Checkpoint::new(1, serde_json::json!({"step": "effect_requested"})),
                binding: "external".to_owned(),
                request: input.command.content().clone(),
                recovery: self.recovery,
            }),
            Some(_effect) if self.fail_after_effect => {
                Err(LoopError::new("injected post-settlement failure"))
            }
            Some(effect) => Ok(LoopDecision::Complete {
                checkpoint: Checkpoint::new(1, serde_json::json!({"step": "done"})),
                events: vec![NewEvent::new(
                    "effect_result",
                    serde_json::to_value(effect.outcome).expect("serialize outcome"),
                )],
            }),
        }
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
        let effect = &snapshot.operations[0].effects[0];
        assert_eq!(effect.status, EffectStatus::DispatchStarted);
        assert_eq!(effect.request, self.expected_request);
        assert_eq!(effect.dispatch_count, 1);
        *self.observed.lock().expect("observation lock") = true;
        Box::pin(std::future::ready(
            EffectOutcome::Success(serde_json::json!({"result": "ok"})).into(),
        ))
    }
}
