use std::{
    sync::{
        Arc, Mutex,
        atomic::{AtomicUsize, Ordering},
    },
    time::Duration,
};

use renoa_kernel::{
    AgentId, CancellationId, CancellationInput, CancellationTransition, Checkpoint, Command,
    CommandId, DriveResult, EffectAdapter, EffectBatchFacts, EffectBinding, EffectCompletion,
    EffectFact, EffectFuture, EffectInvocation, EffectOutcome, EffectRecovery, EffectRequest,
    EffectStatus, Kernel, KernelError, LoopBinding, LoopDecision, LoopError, LoopInput, LoopPlugin,
    NewEvent, OperationOutcome, OperationStatus, Runtime, SessionId, UnknownEffectAbandonment,
    UnknownEffectInput,
};
use tempfile::tempdir;
use tokio::sync::{Barrier, Notify};

#[tokio::test]
async fn effects_run_concurrently_and_return_in_declaration_order() {
    let directory = tempdir().expect("temporary directory");
    let kernel = Kernel::open(directory.path().join("kernel.sqlite3")).expect("open kernel");
    let agent_id = AgentId::new();
    let session_id = SessionId::new();
    kernel.create_agent(agent_id).expect("create agent");
    kernel
        .create_session(session_id, agent_id)
        .expect("create session");
    let admission = kernel
        .submit(
            session_id,
            Command::new(CommandId::new(), serde_json::json!({"batch": true})),
        )
        .expect("submit command");

    let completion_order = Arc::new(Mutex::new(Vec::new()));
    let barrier = Arc::new(Barrier::new(2));
    let right_completed = Arc::new(Notify::new());
    let runtime = Runtime::new(
        LoopBinding::new("batch-loop", "1", Arc::new(BatchLoop)),
        1,
        "batch-config-1",
        vec![
            EffectBinding::new(
                "left",
                "1",
                Arc::new(OrderedAdapter {
                    label: "left",
                    barrier: Arc::clone(&barrier),
                    right_completed: Arc::clone(&right_completed),
                    completion_order: Arc::clone(&completion_order),
                }),
            ),
            EffectBinding::new(
                "right",
                "1",
                Arc::new(OrderedAdapter {
                    label: "right",
                    barrier,
                    right_completed,
                    completion_order: Arc::clone(&completion_order),
                }),
            ),
        ],
    )
    .expect("valid runtime");

    let result = tokio::time::timeout(Duration::from_secs(2), kernel.drive(session_id, &runtime))
        .await
        .expect("both effects must enter their adapters concurrently")
        .expect("drive batch");
    assert_eq!(
        result,
        DriveResult::Finished {
            operation_id: admission.operation_id,
            outcome: OperationOutcome::Completed,
        }
    );
    assert_eq!(
        *completion_order.lock().expect("completion order lock"),
        ["right", "left"]
    );
    let events = kernel
        .events_after(session_id, renoa_kernel::EventCursor::START)
        .expect("read events");
    assert_eq!(
        events.events[0].payload,
        serde_json::json!(["left", "right"]),
        "loop delivery follows declaration order, not completion order"
    );
}

#[tokio::test]
async fn invalid_member_rejects_the_whole_batch_without_dispatch_or_residue() {
    let directory = tempdir().expect("temporary directory");
    let kernel = Kernel::open(directory.path().join("kernel.sqlite3")).expect("open kernel");
    let session_id = create_session(&kernel);
    kernel
        .submit(
            session_id,
            Command::new(CommandId::new(), serde_json::json!({"batch": "invalid"})),
        )
        .expect("submit command");
    let calls = Arc::new(AtomicUsize::new(0));
    let runtime = Runtime::new(
        LoopBinding::new("invalid-batch-loop", "1", Arc::new(InvalidBatchLoop)),
        1,
        "invalid-batch-config-1",
        vec![EffectBinding::new(
            "valid",
            "1",
            Arc::new(CountingAdapter(Arc::clone(&calls))),
        )],
    )
    .expect("valid runtime");

    assert!(matches!(
        kernel.drive(session_id, &runtime).await,
        Err(KernelError::EffectBindingUnavailable(binding)) if binding == "missing"
    ));
    assert_eq!(calls.load(Ordering::SeqCst), 0);
    let snapshot = kernel.inspect(session_id).expect("inspect rejected batch");
    assert_eq!(snapshot.operations[0].status, OperationStatus::Running);
    assert!(snapshot.operations[0].effect_batches.is_empty());
}

#[tokio::test]
async fn a_live_unknown_retries_only_that_child_with_stable_identity() {
    let directory = tempdir().expect("temporary directory");
    let kernel = Kernel::open(directory.path().join("kernel.sqlite3")).expect("open kernel");
    let session_id = create_session(&kernel);
    kernel
        .submit(
            session_id,
            Command::new(CommandId::new(), serde_json::json!({"batch": "retry"})),
        )
        .expect("submit command");
    let left_calls = Arc::new(Mutex::new(Vec::new()));
    let right_calls = Arc::new(Mutex::new(Vec::new()));
    let runtime = Runtime::new(
        LoopBinding::new("batch-loop", "1", Arc::new(BatchLoop)),
        1,
        "retry-batch-config-1",
        vec![
            EffectBinding::new(
                "left",
                "1",
                Arc::new(RecordingSuccessAdapter {
                    label: "left",
                    calls: Arc::clone(&left_calls),
                }),
            ),
            EffectBinding::new(
                "right",
                "1",
                Arc::new(UnknownThenSuccessAdapter {
                    label: "right",
                    calls: Arc::clone(&right_calls),
                }),
            ),
        ],
    )
    .expect("valid runtime");

    assert!(matches!(
        kernel.drive(session_id, &runtime).await,
        Ok(DriveResult::Finished {
            outcome: OperationOutcome::Completed,
            ..
        })
    ));
    let left_calls = left_calls.lock().expect("left calls");
    let right_calls = right_calls.lock().expect("right calls");
    assert_eq!(left_calls.len(), 1, "a settled sibling must not replay");
    assert_eq!(
        right_calls.len(),
        2,
        "the unknown child gets one live replay"
    );
    assert_eq!(right_calls[0].effect_id, right_calls[1].effect_id);
    assert_eq!(right_calls[0].batch_id, right_calls[1].batch_id);
    assert_eq!(left_calls[0].batch_id, right_calls[0].batch_id);
    assert_ne!(left_calls[0].effect_id, right_calls[0].effect_id);
    drop(left_calls);
    drop(right_calls);

    let snapshot = kernel.inspect(session_id).expect("inspect retried batch");
    assert_eq!(snapshot.operations[0].effect_batches.len(), 1);
    let batch = &snapshot.operations[0].effect_batches[0];
    assert_eq!(batch.effects.len(), 2);
    assert_eq!(batch.effects[0].dispatch_count, 1);
    assert_eq!(batch.effects[1].dispatch_count, 2);
    assert!(
        batch
            .effects
            .iter()
            .all(|effect| effect.status == EffectStatus::Settled)
    );
}

#[tokio::test]
async fn abandonment_receives_settled_and_unknown_children_in_declaration_order() {
    let directory = tempdir().expect("temporary directory");
    let kernel = Kernel::open(directory.path().join("kernel.sqlite3")).expect("open kernel");
    let session_id = create_session(&kernel);
    let admission = kernel
        .submit(
            session_id,
            Command::new(CommandId::new(), serde_json::json!({"batch": "mixed"})),
        )
        .expect("submit command");
    let abandonment = Arc::new(Mutex::new(None));
    let runtime = Runtime::new(
        LoopBinding::new(
            "mixed-batch-loop",
            "1",
            Arc::new(MixedBatchLoop {
                abandonment: Arc::clone(&abandonment),
            }),
        ),
        1,
        "mixed-batch-config-1",
        vec![
            EffectBinding::new(
                "settled",
                "1",
                Arc::new(FixedAdapter(EffectCompletion::Settled(
                    EffectOutcome::Success(serde_json::json!({"label": "settled"})),
                ))),
            ),
            EffectBinding::new(
                "unknown",
                "1",
                Arc::new(FixedAdapter(EffectCompletion::OutcomeUnknown)),
            ),
        ],
    )
    .expect("valid runtime");

    assert_eq!(
        kernel
            .drive(session_id, &runtime)
            .await
            .expect("drive mixed batch"),
        DriveResult::Blocked {
            operation_id: admission.operation_id,
        }
    );
    let snapshot = kernel.inspect(session_id).expect("inspect mixed batch");
    let batch_id = snapshot.operations[0].effect_batches[0].batch_id;
    assert_eq!(
        snapshot.operations[0].status,
        OperationStatus::OutcomeUnknown
    );
    assert_eq!(
        snapshot.operations[0].effect_batches[0]
            .effects
            .iter()
            .map(|effect| effect.status)
            .collect::<Vec<_>>(),
        [EffectStatus::Settled, EffectStatus::OutcomeUnknown]
    );

    kernel
        .abandon_unknown_effect(session_id, admission.operation_id, &runtime)
        .expect("abandon mixed batch");
    let recorded = abandonment
        .lock()
        .expect("abandonment lock")
        .clone()
        .expect("abandonment input");
    assert_eq!(recorded.batch_id, batch_id);
    assert!(matches!(recorded.effects[0], EffectFact::Settled(_)));
    assert!(matches!(recorded.effects[1], EffectFact::OutcomeUnknown(_)));
}

#[tokio::test]
async fn cancellation_receives_each_childs_exact_persisted_state() {
    let directory = tempdir().expect("temporary directory");
    let kernel =
        Arc::new(Kernel::open(directory.path().join("kernel.sqlite3")).expect("open kernel"));
    let session_id = create_session(&kernel);
    let admission = kernel
        .submit(
            session_id,
            Command::new(CommandId::new(), serde_json::json!({"batch": "cancel"})),
        )
        .expect("submit command");
    let slow_invoked = Arc::new(Notify::new());
    let cancellation = Arc::new(Mutex::new(None));
    let runtime = Arc::new(
        Runtime::new(
            LoopBinding::new(
                "batch-cancellation-loop",
                "1",
                Arc::new(BatchCancellationLoop {
                    cancellation: Arc::clone(&cancellation),
                }),
            ),
            1,
            "batch-cancellation-config-1",
            vec![
                EffectBinding::new(
                    "fast",
                    "1",
                    Arc::new(FixedAdapter(EffectCompletion::Settled(
                        EffectOutcome::Success(serde_json::json!({"label": "fast"})),
                    ))),
                ),
                EffectBinding::new(
                    "slow",
                    "1",
                    Arc::new(CancelledUnknownAdapter {
                        invoked: Arc::clone(&slow_invoked),
                    }),
                ),
            ],
        )
        .expect("valid runtime"),
    );
    let runner = Arc::clone(&kernel);
    let driven_runtime = Arc::clone(&runtime);
    let drive =
        tokio::spawn(async move { runner.drive(session_id, driven_runtime.as_ref()).await });
    slow_invoked.notified().await;
    tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            let snapshot = kernel.inspect(session_id).expect("inspect running batch");
            if snapshot.operations[0].effect_batches[0].effects[0].status == EffectStatus::Settled {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("fast child must settle while slow child remains in flight");

    kernel
        .request_cancellation(session_id, admission.operation_id, CancellationId::new())
        .expect("request cancellation");
    assert_eq!(
        drive
            .await
            .expect("join drive")
            .expect("close cancellation"),
        DriveResult::Finished {
            operation_id: admission.operation_id,
            outcome: OperationOutcome::Cancelled,
        }
    );
    let recorded = cancellation
        .lock()
        .expect("cancellation lock")
        .clone()
        .expect("cancellation input");
    assert!(matches!(recorded.effects[0], EffectFact::Settled(_)));
    assert!(matches!(recorded.effects[1], EffectFact::OutcomeUnknown(_)));
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

struct BatchLoop;

impl LoopPlugin for BatchLoop {
    fn decide(&self, input: LoopInput) -> Result<LoopDecision, LoopError> {
        let Some(batch) = input.effect_batch else {
            return Ok(LoopDecision::InvokeEffects {
                checkpoint: Checkpoint::new(1, serde_json::json!({"step": "batch"})),
                effects: vec![
                    EffectRequest {
                        binding: "left".to_owned(),
                        request: serde_json::json!({"label": "left"}),
                        recovery: EffectRecovery::SafeToReplay,
                    },
                    EffectRequest {
                        binding: "right".to_owned(),
                        request: serde_json::json!({"label": "right"}),
                        recovery: EffectRecovery::SafeToReplay,
                    },
                ],
            });
        };
        let labels = batch
            .effects
            .into_iter()
            .map(|effect| match effect.outcome {
                EffectOutcome::Success(value) => {
                    value["label"].as_str().expect("label outcome").to_owned()
                }
                EffectOutcome::Failure { message } => panic!("unexpected failure: {message}"),
                other => panic!("unexpected outcome: {other:?}"),
            })
            .collect::<Vec<_>>();
        Ok(LoopDecision::Complete {
            checkpoint: Checkpoint::new(1, serde_json::json!({"step": "done"})),
            events: vec![NewEvent::new("batch_result", serde_json::json!(labels))],
        })
    }
}

struct InvalidBatchLoop;

impl LoopPlugin for InvalidBatchLoop {
    fn decide(&self, _input: LoopInput) -> Result<LoopDecision, LoopError> {
        Ok(LoopDecision::InvokeEffects {
            checkpoint: Checkpoint::new(1, serde_json::json!({"step": "invalid"})),
            effects: vec![
                EffectRequest {
                    binding: "valid".to_owned(),
                    request: serde_json::json!({"position": 0}),
                    recovery: EffectRecovery::SafeToReplay,
                },
                EffectRequest {
                    binding: "missing".to_owned(),
                    request: serde_json::json!({"position": 1}),
                    recovery: EffectRecovery::SafeToReplay,
                },
            ],
        })
    }
}

struct MixedBatchLoop {
    abandonment: Arc<Mutex<Option<renoa_kernel::EffectBatchFacts>>>,
}

struct BatchCancellationLoop {
    cancellation: Arc<Mutex<Option<EffectBatchFacts>>>,
}

impl LoopPlugin for BatchCancellationLoop {
    fn decide(&self, _input: LoopInput) -> Result<LoopDecision, LoopError> {
        Ok(LoopDecision::InvokeEffects {
            checkpoint: Checkpoint::new(1, serde_json::json!({"step": "cancelling"})),
            effects: vec![
                EffectRequest {
                    binding: "fast".to_owned(),
                    request: serde_json::json!({"position": 0}),
                    recovery: EffectRecovery::NeverReplay,
                },
                EffectRequest {
                    binding: "slow".to_owned(),
                    request: serde_json::json!({"position": 1}),
                    recovery: EffectRecovery::NeverReplay,
                },
            ],
        })
    }

    fn cancel_operation(
        &self,
        input: CancellationInput,
    ) -> Result<CancellationTransition, LoopError> {
        *self.cancellation.lock().expect("cancellation lock") = input.effect_batch;
        Ok(CancellationTransition {
            checkpoint: Checkpoint::new(1, serde_json::json!({"step": "cancelled"})),
            events: Vec::new(),
        })
    }
}

impl LoopPlugin for MixedBatchLoop {
    fn decide(&self, _input: LoopInput) -> Result<LoopDecision, LoopError> {
        Ok(LoopDecision::InvokeEffects {
            checkpoint: Checkpoint::new(1, serde_json::json!({"step": "mixed"})),
            effects: vec![
                EffectRequest {
                    binding: "settled".to_owned(),
                    request: serde_json::json!({"position": 0}),
                    recovery: EffectRecovery::NeverReplay,
                },
                EffectRequest {
                    binding: "unknown".to_owned(),
                    request: serde_json::json!({"position": 1}),
                    recovery: EffectRecovery::NeverReplay,
                },
            ],
        })
    }

    fn abandon_unknown_effect(
        &self,
        input: UnknownEffectInput,
    ) -> Result<UnknownEffectAbandonment, LoopError> {
        *self.abandonment.lock().expect("abandonment lock") = Some(input.effect_batch);
        Ok(UnknownEffectAbandonment {
            checkpoint: Checkpoint::new(1, serde_json::json!({"step": "abandoned"})),
            events: Vec::new(),
        })
    }
}

struct OrderedAdapter {
    label: &'static str,
    barrier: Arc<Barrier>,
    right_completed: Arc<Notify>,
    completion_order: Arc<Mutex<Vec<&'static str>>>,
}

impl EffectAdapter for OrderedAdapter {
    fn invoke(&self, _invocation: EffectInvocation) -> EffectFuture<'_> {
        Box::pin(async move {
            let right_completed = self.right_completed.notified();
            self.barrier.wait().await;
            if self.label == "left" {
                right_completed.await;
            }
            self.completion_order
                .lock()
                .expect("completion order lock")
                .push(self.label);
            if self.label == "right" {
                self.right_completed.notify_one();
            }
            EffectOutcome::Success(serde_json::json!({"label": self.label})).into()
        })
    }
}

struct CountingAdapter(Arc<AtomicUsize>);

impl EffectAdapter for CountingAdapter {
    fn invoke(&self, _invocation: EffectInvocation) -> EffectFuture<'_> {
        self.0.fetch_add(1, Ordering::SeqCst);
        Box::pin(std::future::ready(
            EffectOutcome::Success(serde_json::json!({"ok": true})).into(),
        ))
    }
}

struct RecordingSuccessAdapter {
    label: &'static str,
    calls: Arc<Mutex<Vec<EffectInvocation>>>,
}

impl EffectAdapter for RecordingSuccessAdapter {
    fn invoke(&self, invocation: EffectInvocation) -> EffectFuture<'_> {
        self.calls.lock().expect("calls lock").push(invocation);
        Box::pin(std::future::ready(
            EffectOutcome::Success(serde_json::json!({"label": self.label})).into(),
        ))
    }
}

struct UnknownThenSuccessAdapter {
    label: &'static str,
    calls: Arc<Mutex<Vec<EffectInvocation>>>,
}

impl EffectAdapter for UnknownThenSuccessAdapter {
    fn invoke(&self, invocation: EffectInvocation) -> EffectFuture<'_> {
        let attempt = {
            let mut calls = self.calls.lock().expect("calls lock");
            calls.push(invocation);
            calls.len()
        };
        Box::pin(std::future::ready(if attempt == 1 {
            EffectCompletion::OutcomeUnknown
        } else {
            EffectCompletion::Settled(EffectOutcome::Success(
                serde_json::json!({"label": self.label}),
            ))
        }))
    }
}

struct FixedAdapter(EffectCompletion);

impl EffectAdapter for FixedAdapter {
    fn invoke(&self, _invocation: EffectInvocation) -> EffectFuture<'_> {
        Box::pin(std::future::ready(self.0.clone()))
    }
}

struct CancelledUnknownAdapter {
    invoked: Arc<Notify>,
}

impl EffectAdapter for CancelledUnknownAdapter {
    fn invoke(&self, invocation: EffectInvocation) -> EffectFuture<'_> {
        Box::pin(async move {
            self.invoked.notify_one();
            invocation.cancellation.cancelled().await;
            EffectCompletion::OutcomeUnknown
        })
    }
}
