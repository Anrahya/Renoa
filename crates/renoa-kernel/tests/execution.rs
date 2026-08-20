use std::sync::{Arc, Mutex};

use renoa_kernel::{
    AgentId, Checkpoint, Command, CommandId, DriveResult, EventCursor, Kernel, KernelError,
    LoopBinding, LoopDecision, LoopError, LoopInput, LoopPlugin, NewEvent, OperationOutcome,
    OperationStatus, Runtime, RuntimeManifest, SessionId,
};
use tempfile::tempdir;

#[tokio::test]
async fn queued_commands_activate_and_finish_in_position_order() {
    let directory = tempdir().expect("temporary directory");
    let kernel = Kernel::open(directory.path().join("kernel.sqlite3")).expect("open kernel");
    let session_id = create_session(&kernel);
    let first = kernel
        .submit(
            session_id,
            Command::new(CommandId::new(), serde_json::json!({"prompt": "first"})),
        )
        .expect("submit first command");
    let second = kernel
        .submit(
            session_id,
            Command::new(CommandId::new(), serde_json::json!({"prompt": "second"})),
        )
        .expect("submit second command");
    let runtime = completing_runtime("config-1", 1);

    assert_eq!(
        kernel
            .drive(session_id, &runtime)
            .await
            .expect("drive first operation"),
        DriveResult::Finished {
            operation_id: first.operation_id,
            outcome: OperationOutcome::Completed,
        }
    );
    let after_first = kernel.inspect(session_id).expect("inspect after first");
    assert_eq!(after_first.operations[0].status, OperationStatus::Completed);
    assert_eq!(after_first.operations[1].status, OperationStatus::Queued);
    let first_page = kernel
        .events_after(session_id, EventCursor::START)
        .expect("read first event");
    assert_eq!(first_page.events.len(), 1);
    assert_eq!(first_page.events[0].payload, serde_json::json!("first"));

    assert!(matches!(
        kernel
            .drive(session_id, &runtime)
            .await
            .expect("drive second operation"),
        DriveResult::Finished { operation_id, .. } if operation_id == second.operation_id
    ));
    let all = kernel
        .events_after(session_id, EventCursor::START)
        .expect("read all events");
    assert_eq!(
        all.events
            .iter()
            .map(|event| event.payload.clone())
            .collect::<Vec<_>>(),
        vec![serde_json::json!("first"), serde_json::json!("second")]
    );
}

#[tokio::test]
async fn activation_freezes_the_exact_runtime_manifest() {
    let directory = tempdir().expect("temporary directory");
    let database = directory.path().join("kernel.sqlite3");
    let kernel = Kernel::open(&database).expect("open kernel");
    let session_id = create_session(&kernel);
    kernel
        .submit(
            session_id,
            Command::new(CommandId::new(), serde_json::json!({"prompt": "work"})),
        )
        .expect("submit command");
    let failing = Runtime::new(
        LoopBinding::new("scripted", "1", Arc::new(FailingLoop)),
        1,
        "config-1",
        Vec::new(),
    )
    .expect("valid runtime");

    assert!(matches!(
        kernel.drive(session_id, &failing).await,
        Err(KernelError::Loop(error)) if error.message() == "injected loop failure"
    ));
    let frozen = kernel
        .inspect(session_id)
        .expect("inspect active operation");
    assert_eq!(
        frozen.operations[0].manifest.as_ref(),
        Some(failing.manifest())
    );
    drop(kernel);

    let kernel = Kernel::open(&database).expect("reopen kernel");
    let changed = completing_runtime("config-2", 1);
    assert!(matches!(
        kernel.drive(session_id, &changed).await,
        Err(KernelError::RuntimeMismatch)
    ));
    let resumed = completing_runtime("config-1", 1);
    assert!(matches!(
        kernel
            .drive(session_id, &resumed)
            .await
            .expect("resume exact runtime"),
        DriveResult::Finished { .. }
    ));
}

#[tokio::test]
async fn loop_receives_the_exact_frozen_runtime_manifest() {
    let directory = tempdir().expect("temporary directory");
    let kernel = Kernel::open(directory.path().join("kernel.sqlite3")).expect("open kernel");
    let session_id = create_session(&kernel);
    kernel
        .submit(
            session_id,
            Command::new(CommandId::new(), serde_json::json!({"prompt": "work"})),
        )
        .expect("submit command");
    let observed = Arc::new(Mutex::new(None));
    let runtime = Runtime::new(
        LoopBinding::new(
            "manifest-observer",
            "3",
            Arc::new(ManifestObservingLoop {
                observed: Arc::clone(&observed),
            }),
        ),
        7,
        "manifest-config",
        Vec::new(),
    )
    .expect("valid runtime");

    kernel
        .drive(session_id, &runtime)
        .await
        .expect("drive operation");

    assert_eq!(
        observed.lock().expect("observed manifest lock").as_ref(),
        Some(runtime.manifest())
    );
}

#[tokio::test]
async fn a_wrong_checkpoint_schema_cannot_advance_durable_state() {
    let directory = tempdir().expect("temporary directory");
    let kernel = Kernel::open(directory.path().join("kernel.sqlite3")).expect("open kernel");
    let session_id = create_session(&kernel);
    kernel
        .submit(
            session_id,
            Command::new(CommandId::new(), serde_json::json!({"prompt": "work"})),
        )
        .expect("submit command");
    let wrong = completing_runtime("config-1", 2);

    assert!(matches!(
        kernel.drive(session_id, &wrong).await,
        Err(KernelError::CheckpointSchemaMismatch {
            expected: 1,
            found: 2,
        })
    ));
    let snapshot = kernel.inspect(session_id).expect("inspect unchanged state");
    assert_eq!(snapshot.operations[0].status, OperationStatus::Running);
    assert!(snapshot.operations[0].checkpoint.is_none());
    assert!(
        kernel
            .events_after(session_id, EventCursor::START)
            .expect("read events")
            .events
            .is_empty()
    );

    assert!(matches!(
        kernel
            .drive(session_id, &completing_runtime("config-1", 1))
            .await
            .expect("retry valid decision"),
        DriveResult::Finished { .. }
    ));
}

#[tokio::test]
async fn append_wait_and_fail_decisions_have_explicit_durable_outcomes() {
    let directory = tempdir().expect("temporary directory");
    let kernel = Kernel::open(directory.path().join("kernel.sqlite3")).expect("open kernel");
    let session_id = create_session(&kernel);
    kernel
        .submit(
            session_id,
            Command::new(CommandId::new(), serde_json::json!("wait")),
        )
        .expect("submit waiting command");
    kernel
        .submit(
            session_id,
            Command::new(CommandId::new(), serde_json::json!("fail")),
        )
        .expect("submit failing command");
    let waiting = Runtime::new(
        LoopBinding::new("waiting", "1", Arc::new(WaitingLoop)),
        1,
        "waiting-config-1",
        Vec::new(),
    )
    .expect("valid waiting runtime");
    let failed = Runtime::new(
        LoopBinding::new("failing", "1", Arc::new(FailingDecisionLoop)),
        1,
        "failing-config-1",
        Vec::new(),
    )
    .expect("valid failing runtime");

    assert!(matches!(
        kernel
            .drive(session_id, &waiting)
            .await
            .expect("drive waiting operation"),
        DriveResult::Finished {
            outcome: OperationOutcome::WaitingForInput,
            ..
        }
    ));
    assert!(matches!(
        kernel
            .drive(session_id, &failed)
            .await
            .expect("drive failing operation"),
        DriveResult::Finished {
            outcome: OperationOutcome::Failed { ref reason },
            ..
        } if reason == "scripted failure"
    ));
    let snapshot = kernel.inspect(session_id).expect("inspect outcomes");
    assert_eq!(snapshot.operations[0].status, OperationStatus::Waiting);
    assert_eq!(snapshot.operations[1].status, OperationStatus::Failed);
    assert_eq!(
        kernel
            .events_after(session_id, EventCursor::START)
            .expect("read decision events")
            .events
            .iter()
            .map(|event| event.kind.as_str())
            .collect::<Vec<_>>(),
        vec!["progress", "waiting", "failed"]
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

fn completing_runtime(config_digest: &str, checkpoint_schema: u32) -> Runtime {
    Runtime::new(
        LoopBinding::new(
            "scripted",
            "1",
            Arc::new(CompletingLoop {
                checkpoint_schema,
                seen: Mutex::new(Vec::new()),
            }),
        ),
        1,
        config_digest,
        Vec::new(),
    )
    .expect("valid runtime")
}

struct CompletingLoop {
    checkpoint_schema: u32,
    seen: Mutex<Vec<serde_json::Value>>,
}

struct ManifestObservingLoop {
    observed: Arc<Mutex<Option<RuntimeManifest>>>,
}

impl LoopPlugin for ManifestObservingLoop {
    fn decide(&self, input: LoopInput) -> Result<LoopDecision, LoopError> {
        *self.observed.lock().expect("observed manifest lock") = Some(input.runtime_manifest);
        Ok(LoopDecision::Complete {
            checkpoint: Checkpoint::new(7, serde_json::json!({"done": true})),
            events: Vec::new(),
        })
    }
}

impl LoopPlugin for CompletingLoop {
    fn decide(&self, input: LoopInput) -> Result<LoopDecision, LoopError> {
        let prompt = input.command.content()["prompt"].clone();
        self.seen.lock().expect("seen lock").push(prompt.clone());
        Ok(LoopDecision::Complete {
            checkpoint: Checkpoint::new(self.checkpoint_schema, serde_json::json!({"done": true})),
            events: vec![NewEvent::new("answer", prompt)],
        })
    }
}

struct FailingLoop;

impl LoopPlugin for FailingLoop {
    fn decide(&self, _input: LoopInput) -> Result<LoopDecision, LoopError> {
        Err(LoopError::new("injected loop failure"))
    }
}

struct WaitingLoop;

impl LoopPlugin for WaitingLoop {
    fn decide(&self, input: LoopInput) -> Result<LoopDecision, LoopError> {
        if input.checkpoint.is_none() {
            Ok(LoopDecision::AppendEventsAndContinue {
                checkpoint: Checkpoint::new(1, serde_json::json!({"step": 1})),
                events: vec![NewEvent::new("progress", serde_json::json!(1))],
            })
        } else {
            Ok(LoopDecision::WaitForInput {
                checkpoint: Checkpoint::new(1, serde_json::json!({"step": 2})),
                events: vec![NewEvent::new("waiting", serde_json::json!(true))],
            })
        }
    }
}

struct FailingDecisionLoop;

impl LoopPlugin for FailingDecisionLoop {
    fn decide(&self, _input: LoopInput) -> Result<LoopDecision, LoopError> {
        Ok(LoopDecision::Fail {
            checkpoint: Checkpoint::new(1, serde_json::json!({"failed": true})),
            events: vec![NewEvent::new("failed", serde_json::json!(true))],
            reason: "scripted failure".to_owned(),
        })
    }
}
