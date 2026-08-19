use std::sync::{
    Arc, Mutex,
    atomic::{AtomicUsize, Ordering},
};

use renoa_kernel::{
    AgentId, Checkpoint, Command, CommandId, DriveResult, EffectAdapter, EffectBinding,
    EffectCompletion, EffectFuture, EffectInvocation, EffectRecovery, EffectStatus, EventCursor,
    Kernel, KernelError, LoopBinding, LoopDecision, LoopError, LoopInput, LoopPlugin, NewEvent,
    OperationOutcome, OperationStatus, Runtime, SessionId, UnknownEffectAbandonment,
    UnknownEffectInput,
};
use tempfile::tempdir;

const ABANDONED_REASON: &str = "effect outcome is unknown; operation was abandoned without replay";

#[tokio::test]
async fn abandonment_is_atomic_idempotent_and_unblocks_queued_work() {
    let directory = tempdir().expect("temporary directory");
    let kernel = Kernel::open(directory.path().join("kernel.sqlite3")).expect("open kernel");
    let session_id = create_session(&kernel);
    let blocked = kernel
        .submit(
            session_id,
            Command::new(CommandId::new(), serde_json::json!({"effect": true})),
        )
        .expect("submit effect command");
    let queued = kernel
        .submit(
            session_id,
            Command::new(CommandId::new(), serde_json::json!({"effect": false})),
        )
        .expect("submit queued command");
    let abandon_calls = Arc::new(AtomicUsize::new(0));
    let adapter_calls = Arc::new(AtomicUsize::new(0));
    let main_runtime = runtime(
        "config-v1",
        Arc::clone(&abandon_calls),
        Arc::clone(&adapter_calls),
    );

    assert_eq!(
        kernel
            .drive(session_id, &main_runtime)
            .await
            .expect("drive unknown effect"),
        DriveResult::Blocked {
            operation_id: blocked.operation_id,
        }
    );

    assert_rejected_attempts(
        &kernel,
        session_id,
        blocked.operation_id,
        &main_runtime,
        &abandon_calls,
        &adapter_calls,
    );

    let expected = OperationOutcome::Failed {
        reason: ABANDONED_REASON.to_owned(),
    };
    assert_eq!(
        kernel
            .abandon_unknown_effect(session_id, blocked.operation_id, &main_runtime)
            .expect("abandon unknown effect"),
        expected
    );
    assert_eq!(
        kernel
            .abandon_unknown_effect(session_id, blocked.operation_id, &main_runtime)
            .expect("retry abandonment after lost reply"),
        expected
    );
    assert_eq!(abandon_calls.load(Ordering::SeqCst), 1);
    assert_eq!(adapter_calls.load(Ordering::SeqCst), 1);

    let snapshot = kernel.inspect(session_id).expect("inspect abandonment");
    assert_eq!(snapshot.operations[0].status, OperationStatus::Failed);
    assert_eq!(snapshot.operations[0].outcome, Some(expected));
    assert_eq!(
        snapshot.operations[0].effects[0].status,
        EffectStatus::OutcomeUnknown
    );
    assert_eq!(snapshot.operations[0].effects[0].outcome, None);
    let events = kernel
        .events_after(session_id, EventCursor::START)
        .expect("read abandonment events")
        .events;
    assert_eq!(events.len(), 2);
    assert_eq!(events[0].kind, "started");
    assert_eq!(events[1].kind, "unknown_effect_abandoned");

    assert!(matches!(
        kernel
            .drive(session_id, &main_runtime)
            .await
            .expect("drive queued work"),
        DriveResult::Finished {
            operation_id,
            outcome: OperationOutcome::Completed,
        } if operation_id == queued.operation_id
    ));
    assert_eq!(adapter_calls.load(Ordering::SeqCst), 1);
    assert!(matches!(
        kernel.abandon_unknown_effect(session_id, queued.operation_id, &main_runtime),
        Err(KernelError::NoUnknownEffect(operation_id)) if operation_id == queued.operation_id
    ));
}

#[tokio::test]
async fn corrupted_history_prevents_abandonment_before_the_loop_runs() {
    let directory = tempdir().expect("temporary directory");
    let database = directory.path().join("kernel.sqlite3");
    let kernel = Kernel::open(&database).expect("open kernel");
    let session_id = create_session(&kernel);
    let blocked = kernel
        .submit(
            session_id,
            Command::new(CommandId::new(), serde_json::json!({"effect": true})),
        )
        .expect("submit command");
    let abandon_calls = Arc::new(AtomicUsize::new(0));
    let runtime = runtime(
        "config-v1",
        Arc::clone(&abandon_calls),
        Arc::new(AtomicUsize::new(0)),
    );
    assert!(matches!(
        kernel.drive(session_id, &runtime).await,
        Ok(DriveResult::Blocked { .. })
    ));
    drop(kernel);

    let connection = rusqlite::Connection::open(&database).expect("open raw database");
    connection
        .execute(
            "DELETE FROM semantic_events WHERE session_id = ?1 AND sequence = 0",
            [session_id.to_string()],
        )
        .expect("remove event without moving high-water mark");
    drop(connection);

    let kernel = Kernel::open(&database).expect("reopen kernel");
    assert!(matches!(
        kernel.abandon_unknown_effect(session_id, blocked.operation_id, &runtime),
        Err(KernelError::Corrupt(_))
    ));
    assert_eq!(abandon_calls.load(Ordering::SeqCst), 0);
    let snapshot = kernel
        .inspect(session_id)
        .expect("inspect blocked operation");
    assert_eq!(
        snapshot.operations[0].status,
        OperationStatus::OutcomeUnknown
    );
    assert_eq!(snapshot.operations[0].outcome, None);
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

fn assert_rejected_attempts(
    kernel: &Kernel,
    session_id: SessionId,
    operation_id: renoa_kernel::OperationId,
    main_runtime: &Runtime,
    abandon_calls: &Arc<AtomicUsize>,
    adapter_calls: &Arc<AtomicUsize>,
) {
    let incompatible = runtime(
        "different-config",
        Arc::clone(abandon_calls),
        Arc::clone(adapter_calls),
    );
    assert!(matches!(
        kernel.abandon_unknown_effect(session_id, operation_id, &incompatible),
        Err(KernelError::RuntimeMismatch)
    ));
    assert_eq!(abandon_calls.load(Ordering::SeqCst), 0);

    let unsupported = unsupported_runtime(Arc::clone(adapter_calls));
    assert!(matches!(
        kernel.abandon_unknown_effect(session_id, operation_id, &unsupported),
        Err(KernelError::Loop(error))
            if error.message() == "loop plugin does not support unknown-effect abandonment"
    ));
    assert_eq!(
        kernel
            .inspect(session_id)
            .expect("inspect failed abandonment")
            .operations[0]
            .status,
        OperationStatus::OutcomeUnknown
    );
    assert_eq!(abandon_calls.load(Ordering::SeqCst), 0);

    let other_session = create_session(kernel);
    assert!(matches!(
        kernel.abandon_unknown_effect(other_session, operation_id, main_runtime),
        Err(KernelError::NoUnknownEffect(found)) if found == operation_id
    ));
}

fn runtime(
    config_digest: &str,
    abandon_calls: Arc<AtomicUsize>,
    adapter_calls: Arc<AtomicUsize>,
) -> Runtime {
    Runtime::new(
        LoopBinding::new(
            "unknown-effect-loop",
            "1",
            Arc::new(TestLoop {
                abandon_calls,
                observed_unknown: Mutex::new(None),
            }),
        ),
        1,
        config_digest,
        vec![EffectBinding::new(
            "external",
            "1",
            Arc::new(UnknownAdapter(adapter_calls)),
        )],
    )
    .expect("valid runtime")
}

fn unsupported_runtime(adapter_calls: Arc<AtomicUsize>) -> Runtime {
    Runtime::new(
        LoopBinding::new("unknown-effect-loop", "1", Arc::new(UnsupportedLoop)),
        1,
        "config-v1",
        vec![EffectBinding::new(
            "external",
            "1",
            Arc::new(UnknownAdapter(adapter_calls)),
        )],
    )
    .expect("valid unsupported runtime")
}

struct UnsupportedLoop;

impl LoopPlugin for UnsupportedLoop {
    fn decide(&self, _input: LoopInput) -> Result<LoopDecision, LoopError> {
        panic!("ordinary loop decision must not run during abandonment")
    }
}

struct TestLoop {
    abandon_calls: Arc<AtomicUsize>,
    observed_unknown: Mutex<Option<renoa_kernel::EffectId>>,
}

impl LoopPlugin for TestLoop {
    fn decide(&self, input: LoopInput) -> Result<LoopDecision, LoopError> {
        if input.command.content()["effect"] == serde_json::json!(false) {
            return Ok(LoopDecision::Complete {
                checkpoint: Checkpoint::new(1, serde_json::json!({"phase": "done"})),
                events: Vec::new(),
            });
        }
        if input.checkpoint.is_none() {
            return Ok(LoopDecision::AppendEventsAndContinue {
                checkpoint: Checkpoint::new(1, serde_json::json!({"phase": "request"})),
                events: vec![NewEvent::new("started", serde_json::json!(true))],
            });
        }
        Ok(LoopDecision::InvokeEffect {
            checkpoint: Checkpoint::new(1, serde_json::json!({"phase": "awaiting"})),
            binding: "external".to_owned(),
            request: input.command.content().clone(),
            recovery: EffectRecovery::NeverReplay,
        })
    }

    fn abandon_unknown_effect(
        &self,
        input: UnknownEffectInput,
    ) -> Result<UnknownEffectAbandonment, LoopError> {
        self.abandon_calls.fetch_add(1, Ordering::SeqCst);
        if input.effect.binding != "external"
            || input.effect.binding_revision != "1"
            || input.effect.request != serde_json::json!({"effect": true})
        {
            return Err(LoopError::new("unknown effect input changed"));
        }
        *self
            .observed_unknown
            .lock()
            .map_err(|error| LoopError::new(format!("observation lock poisoned: {error}")))? =
            Some(input.effect.effect_id);
        Ok(UnknownEffectAbandonment {
            checkpoint: Checkpoint::new(1, serde_json::json!({"phase": "terminal"})),
            events: vec![NewEvent::new(
                "unknown_effect_abandoned",
                serde_json::json!({"effect_id": input.effect.effect_id}),
            )],
        })
    }
}

struct UnknownAdapter(Arc<AtomicUsize>);

impl EffectAdapter for UnknownAdapter {
    fn invoke(&self, _invocation: EffectInvocation) -> EffectFuture<'_> {
        self.0.fetch_add(1, Ordering::SeqCst);
        Box::pin(std::future::ready(EffectCompletion::OutcomeUnknown))
    }
}
