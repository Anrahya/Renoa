use std::sync::{
    Arc, Mutex,
    atomic::{AtomicUsize, Ordering},
    mpsc,
};

use renoa_kernel::{
    AgentId, CancellationEffect, CancellationId, CancellationInput, CancellationTransition,
    Checkpoint, Command, CommandId, DriveResult, EffectAdapter, EffectBinding, EffectFuture,
    EffectInvocation, EffectOutcome, EffectRecovery, EffectStatus, Kernel, KernelError,
    LoopBinding, LoopDecision, LoopError, LoopInput, LoopPlugin, NewEvent, OperationOutcome,
    OperationStatus, Runtime, SessionId,
};
use tempfile::tempdir;
use tokio::sync::Notify;

#[tokio::test]
async fn cancellation_identity_is_exact_idempotent_and_operation_scoped() {
    let directory = tempdir().expect("temporary directory");
    let kernel = Kernel::open(directory.path().join("kernel.sqlite3")).expect("open kernel");
    let session_id = create_session(&kernel);
    let first = kernel
        .submit(
            session_id,
            Command::new(CommandId::new(), serde_json::json!({"mode": "pause"})),
        )
        .expect("submit first command");
    let cancellation_calls = Arc::new(AtomicUsize::new(0));
    let runtime = control_runtime(Arc::clone(&cancellation_calls));

    assert!(matches!(
        kernel.drive(session_id, &runtime).await,
        Err(KernelError::Loop(error)) if error.message() == "pause before cancellation"
    ));
    let cancellation_id = CancellationId::new();
    kernel
        .request_cancellation(session_id, first.operation_id, cancellation_id)
        .expect("persist cancellation");
    kernel
        .request_cancellation(session_id, first.operation_id, cancellation_id)
        .expect("retry cancellation");
    assert_eq!(
        kernel
            .drive(session_id, &runtime)
            .await
            .expect("close cancellation"),
        DriveResult::Finished {
            operation_id: first.operation_id,
            outcome: OperationOutcome::Cancelled,
        }
    );
    assert_eq!(cancellation_calls.load(Ordering::SeqCst), 1);
    kernel
        .request_cancellation(session_id, first.operation_id, cancellation_id)
        .expect("retry settled cancellation");
    assert!(matches!(
        kernel.request_cancellation(
            session_id,
            first.operation_id,
            CancellationId::new()
        ),
        Err(KernelError::OperationNotCancellable(id)) if id == first.operation_id
    ));

    let second = kernel
        .submit(
            session_id,
            Command::new(CommandId::new(), serde_json::json!({"mode": "complete"})),
        )
        .expect("submit second command");
    assert!(matches!(
        kernel.request_cancellation(session_id, second.operation_id, cancellation_id),
        Err(KernelError::CancellationConflict {
            cancellation_id: found,
            operation_id,
        }) if found == cancellation_id && operation_id == first.operation_id
    ));
    kernel
        .request_cancellation(session_id, first.operation_id, cancellation_id)
        .expect("old retry cannot target next operation");
    assert_eq!(
        kernel
            .drive(session_id, &runtime)
            .await
            .expect("drive second operation"),
        DriveResult::Finished {
            operation_id: second.operation_id,
            outcome: OperationOutcome::Completed,
        }
    );
}

#[tokio::test]
async fn cancellation_waits_for_started_effect_cleanup_and_preserves_its_result() {
    let directory = tempdir().expect("temporary directory");
    let kernel =
        Arc::new(Kernel::open(directory.path().join("kernel.sqlite3")).expect("open kernel"));
    let session_id = create_session(&kernel);
    let admission = kernel
        .submit(
            session_id,
            Command::new(CommandId::new(), serde_json::json!({"work": true})),
        )
        .expect("submit command");
    let invoked = Arc::new(Notify::new());
    let cleanup_started = Arc::new(Notify::new());
    let release_cleanup = Arc::new(Notify::new());
    let cancellation_effects = Arc::new(Mutex::new(Vec::new()));
    let runtime = Arc::new(effect_runtime(
        Arc::clone(&invoked),
        Arc::clone(&cleanup_started),
        Arc::clone(&release_cleanup),
        Arc::clone(&cancellation_effects),
    ));
    let runner = Arc::clone(&kernel);
    let driven_runtime = Arc::clone(&runtime);
    let mut drive =
        tokio::spawn(async move { runner.drive(session_id, driven_runtime.as_ref()).await });
    invoked.notified().await;

    kernel
        .request_cancellation(session_id, admission.operation_id, CancellationId::new())
        .expect("persist cancellation");
    cleanup_started.notified().await;
    assert!(
        tokio::time::timeout(std::time::Duration::from_millis(25), &mut drive)
            .await
            .is_err(),
        "drive returned before adapter cleanup finished"
    );
    release_cleanup.notify_one();
    assert_eq!(
        drive
            .await
            .expect("join drive")
            .expect("settle cancellation"),
        DriveResult::Finished {
            operation_id: admission.operation_id,
            outcome: OperationOutcome::Cancelled,
        }
    );

    let effects = cancellation_effects.lock().expect("cancellation effects");
    assert!(matches!(
        effects.as_slice(),
        [CancellationEffect::Settled(effect)]
            if effect.outcome == EffectOutcome::Success(serde_json::json!({"cleaned": true}))
    ));
    drop(effects);
    let snapshot = kernel.inspect(session_id).expect("inspect cancellation");
    assert_eq!(snapshot.operations[0].status, OperationStatus::Cancelled);
    assert_eq!(
        snapshot.operations[0].effects[0].status,
        EffectStatus::Settled
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cancellation_committed_while_the_loop_decides_prevents_effect_intent() {
    let directory = tempdir().expect("temporary directory");
    let kernel =
        Arc::new(Kernel::open(directory.path().join("kernel.sqlite3")).expect("open kernel"));
    let session_id = create_session(&kernel);
    let admission = kernel
        .submit(
            session_id,
            Command::new(CommandId::new(), serde_json::json!({"work": true})),
        )
        .expect("submit command");
    let entered = Arc::new(Notify::new());
    let (release_tx, release_rx) = mpsc::channel();
    let adapter_calls = Arc::new(AtomicUsize::new(0));
    let runtime = Arc::new(
        Runtime::new(
            LoopBinding::new(
                "blocking-loop",
                "1",
                Arc::new(BlockingLoop {
                    entered: Arc::clone(&entered),
                    release: Mutex::new(release_rx),
                }),
            ),
            1,
            "blocking-config-1",
            vec![EffectBinding::new(
                "external",
                "1",
                Arc::new(CountingAdapter(Arc::clone(&adapter_calls))),
            )],
        )
        .expect("valid runtime"),
    );
    let runner = Arc::clone(&kernel);
    let driven_runtime = Arc::clone(&runtime);
    let drive =
        tokio::spawn(async move { runner.drive(session_id, driven_runtime.as_ref()).await });
    entered.notified().await;

    kernel
        .request_cancellation(session_id, admission.operation_id, CancellationId::new())
        .expect("persist cancellation while loop decides");
    release_tx.send(()).expect("release loop decision");
    assert!(matches!(
        drive
            .await
            .expect("join drive")
            .expect("close cancellation"),
        DriveResult::Finished {
            outcome: OperationOutcome::Cancelled,
            ..
        }
    ));
    assert_eq!(adapter_calls.load(Ordering::SeqCst), 0);
    let snapshot = kernel.inspect(session_id).expect("inspect cancellation");
    assert!(snapshot.operations[0].effects.is_empty());
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

fn control_runtime(cancellation_calls: Arc<AtomicUsize>) -> Runtime {
    Runtime::new(
        LoopBinding::new(
            "control-loop",
            "1",
            Arc::new(ControlLoop { cancellation_calls }),
        ),
        1,
        "control-config-1",
        Vec::new(),
    )
    .expect("valid runtime")
}

struct ControlLoop {
    cancellation_calls: Arc<AtomicUsize>,
}

impl LoopPlugin for ControlLoop {
    fn decide(&self, input: LoopInput) -> Result<LoopDecision, LoopError> {
        if input.command.content()["mode"] == "complete" {
            Ok(LoopDecision::Complete {
                checkpoint: terminal_checkpoint(),
                events: Vec::new(),
            })
        } else {
            Err(LoopError::new("pause before cancellation"))
        }
    }

    fn cancel_operation(
        &self,
        _input: CancellationInput,
    ) -> Result<CancellationTransition, LoopError> {
        self.cancellation_calls.fetch_add(1, Ordering::SeqCst);
        Ok(CancellationTransition {
            checkpoint: terminal_checkpoint(),
            events: vec![NewEvent::new("cancelled", serde_json::json!(true))],
        })
    }
}

fn effect_runtime(
    invoked: Arc<Notify>,
    cleanup_started: Arc<Notify>,
    release_cleanup: Arc<Notify>,
    cancellation_effects: Arc<Mutex<Vec<CancellationEffect>>>,
) -> Runtime {
    Runtime::new(
        LoopBinding::new(
            "effect-loop",
            "1",
            Arc::new(EffectLoop {
                cancellation_effects,
            }),
        ),
        1,
        "effect-config-1",
        vec![EffectBinding::new(
            "external",
            "1",
            Arc::new(CleanupAdapter {
                invoked,
                cleanup_started,
                release_cleanup,
            }),
        )],
    )
    .expect("valid runtime")
}

struct EffectLoop {
    cancellation_effects: Arc<Mutex<Vec<CancellationEffect>>>,
}

impl LoopPlugin for EffectLoop {
    fn decide(&self, input: LoopInput) -> Result<LoopDecision, LoopError> {
        if input.effect.is_some() {
            Ok(LoopDecision::Complete {
                checkpoint: terminal_checkpoint(),
                events: Vec::new(),
            })
        } else {
            Ok(LoopDecision::InvokeEffect {
                checkpoint: Checkpoint::new(1, serde_json::json!({"awaiting": true})),
                binding: "external".to_owned(),
                request: input.command.content().clone(),
                recovery: EffectRecovery::NeverReplay,
            })
        }
    }

    fn cancel_operation(
        &self,
        input: CancellationInput,
    ) -> Result<CancellationTransition, LoopError> {
        self.cancellation_effects
            .lock()
            .expect("cancellation effects")
            .push(input.effect.expect("cancellation effect"));
        Ok(CancellationTransition {
            checkpoint: terminal_checkpoint(),
            events: Vec::new(),
        })
    }
}

struct CleanupAdapter {
    invoked: Arc<Notify>,
    cleanup_started: Arc<Notify>,
    release_cleanup: Arc<Notify>,
}

impl EffectAdapter for CleanupAdapter {
    fn invoke(&self, invocation: EffectInvocation) -> EffectFuture<'_> {
        Box::pin(async move {
            self.invoked.notify_one();
            invocation.cancellation.cancelled().await;
            self.cleanup_started.notify_one();
            self.release_cleanup.notified().await;
            EffectOutcome::Success(serde_json::json!({"cleaned": true})).into()
        })
    }
}

struct BlockingLoop {
    entered: Arc<Notify>,
    release: Mutex<mpsc::Receiver<()>>,
}

impl LoopPlugin for BlockingLoop {
    fn decide(&self, input: LoopInput) -> Result<LoopDecision, LoopError> {
        self.entered.notify_one();
        self.release
            .lock()
            .expect("release receiver")
            .recv()
            .expect("receive release");
        Ok(LoopDecision::InvokeEffect {
            checkpoint: Checkpoint::new(1, serde_json::json!({"awaiting": true})),
            binding: "external".to_owned(),
            request: input.command.content().clone(),
            recovery: EffectRecovery::NeverReplay,
        })
    }

    fn cancel_operation(
        &self,
        _input: CancellationInput,
    ) -> Result<CancellationTransition, LoopError> {
        Ok(CancellationTransition {
            checkpoint: terminal_checkpoint(),
            events: Vec::new(),
        })
    }
}

struct CountingAdapter(Arc<AtomicUsize>);

impl EffectAdapter for CountingAdapter {
    fn invoke(&self, _invocation: EffectInvocation) -> EffectFuture<'_> {
        self.0.fetch_add(1, Ordering::SeqCst);
        Box::pin(std::future::ready(
            EffectOutcome::Success(serde_json::json!(true)).into(),
        ))
    }
}

fn terminal_checkpoint() -> Checkpoint {
    Checkpoint::new(1, serde_json::json!({"terminal": true}))
}
