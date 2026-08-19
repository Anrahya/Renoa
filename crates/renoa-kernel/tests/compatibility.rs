use std::sync::Arc;

use renoa_kernel::{
    AgentId, Checkpoint, Command, CommandId, EffectAdapter, EffectBinding, EffectFuture,
    EffectInvocation, EffectOutcome, EffectRecovery, EventCursor, Kernel, KernelError, LoopBinding,
    LoopDecision, LoopError, LoopInput, LoopPlugin, NewEvent, Runtime, SessionId,
};
use tempfile::tempdir;

#[tokio::test]
async fn semantic_replay_is_gapless_stable_and_rejects_an_ahead_cursor() {
    let directory = tempdir().expect("temporary directory");
    let kernel = Kernel::open(directory.path().join("kernel.sqlite3")).expect("open kernel");
    let session_id = create_session(&kernel);
    for value in ["first", "second"] {
        kernel
            .submit(
                session_id,
                Command::new(CommandId::new(), serde_json::json!(value)),
            )
            .expect("submit command");
        kernel
            .drive(session_id, &event_runtime())
            .await
            .expect("drive command");
    }

    let all = kernel
        .events_after(session_id, EventCursor::START)
        .expect("read all events");
    assert_eq!(all.next_cursor, EventCursor::new(2));
    assert_eq!(all.events[0].sequence, 0);
    assert_eq!(all.events[1].sequence, 1);
    assert_eq!(
        kernel
            .events_after(session_id, EventCursor::START)
            .expect("repeat replay")
            .events,
        all.events
    );
    assert_eq!(
        kernel
            .events_after(session_id, EventCursor::new(1))
            .expect("read suffix")
            .events,
        vec![all.events[1].clone()]
    );
    assert!(
        kernel
            .events_after(session_id, EventCursor::new(2))
            .expect("read at high-water")
            .events
            .is_empty()
    );
    assert!(matches!(
        kernel.events_after(session_id, EventCursor::new(3)),
        Err(KernelError::CursorAhead {
            cursor: 3,
            high_water: 2,
        })
    ));
}

#[tokio::test]
async fn a_corrupted_semantic_event_gap_stops_replay_and_execution() {
    let directory = tempdir().expect("temporary directory");
    let database = directory.path().join("kernel.sqlite3");
    let kernel = Kernel::open(&database).expect("open kernel");
    let session_id = create_session(&kernel);
    for value in ["first", "second"] {
        kernel
            .submit(
                session_id,
                Command::new(CommandId::new(), serde_json::json!(value)),
            )
            .expect("submit command");
        kernel
            .drive(session_id, &event_runtime())
            .await
            .expect("drive command");
    }
    kernel
        .submit(
            session_id,
            Command::new(CommandId::new(), serde_json::json!("third")),
        )
        .expect("submit queued command");
    drop(kernel);
    let connection = rusqlite::Connection::open(&database).expect("open raw database");
    connection
        .execute(
            "DELETE FROM semantic_events WHERE session_id = ?1 AND sequence = 0",
            [session_id.to_string()],
        )
        .expect("remove first event");
    drop(connection);

    let kernel = Kernel::open(&database).expect("reopen kernel");
    assert!(matches!(
        kernel.events_after(session_id, EventCursor::START),
        Err(KernelError::Corrupt(_))
    ));
    let called = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let runtime = never_called_runtime(&called);
    assert!(matches!(
        kernel.drive(session_id, &runtime).await,
        Err(KernelError::Corrupt(_))
    ));
    assert!(!called.load(std::sync::atomic::Ordering::SeqCst));
}

#[tokio::test]
async fn a_corrupted_semantic_event_gap_stops_effect_recovery() {
    let directory = tempdir().expect("temporary directory");
    let database = directory.path().join("kernel.sqlite3");
    let kernel = Kernel::open(&database).expect("open kernel");
    let session_id = create_session(&kernel);
    for value in ["first", "second"] {
        kernel
            .submit(
                session_id,
                Command::new(CommandId::new(), serde_json::json!(value)),
            )
            .expect("submit command");
        kernel
            .drive(session_id, &event_runtime())
            .await
            .expect("drive command");
    }
    kernel
        .submit(
            session_id,
            Command::new(CommandId::new(), serde_json::json!("third")),
        )
        .expect("submit effect command");
    let runtime = recoverable_effect_runtime(Arc::new(RequestEffectLoop), Arc::new(CrashingEffect));
    let task = tokio::spawn(async move { kernel.drive(session_id, &runtime).await });
    assert!(task.await.expect_err("injected crash").is_panic());

    let connection = rusqlite::Connection::open(&database).expect("open raw database");
    connection
        .execute(
            "DELETE FROM semantic_events WHERE session_id = ?1 AND sequence = 0",
            [session_id.to_string()],
        )
        .expect("remove first event");
    drop(connection);

    let kernel = Kernel::open(&database).expect("reopen kernel");
    let loop_called = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let effect_called = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let recovery = recoverable_effect_runtime(
        Arc::new(NeverCalledLoop(Arc::clone(&loop_called))),
        Arc::new(NeverCalledEffect(Arc::clone(&effect_called))),
    );
    assert!(matches!(
        kernel.drive(session_id, &recovery).await,
        Err(KernelError::Corrupt(_))
    ));
    assert!(!loop_called.load(std::sync::atomic::Ordering::SeqCst));
    assert!(!effect_called.load(std::sync::atomic::Ordering::SeqCst));
}

#[test]
fn a_command_cannot_cross_its_operation_session() {
    let directory = tempdir().expect("temporary directory");
    let database = directory.path().join("kernel.sqlite3");
    let kernel = Kernel::open(&database).expect("open kernel");
    let source_session = create_session(&kernel);
    let target_session = create_session(&kernel);
    let command_id = CommandId::new();
    kernel
        .submit(
            source_session,
            Command::new(command_id, serde_json::json!("work")),
        )
        .expect("submit command");
    drop(kernel);

    let connection = rusqlite::Connection::open(&database).expect("open raw database");
    connection
        .execute_batch("PRAGMA foreign_keys = ON;")
        .expect("enable foreign keys");
    assert!(
        connection
            .execute(
                "UPDATE commands SET session_id = ?2 WHERE command_id = ?1",
                rusqlite::params![command_id.to_string(), target_session.to_string()],
            )
            .is_err()
    );
    connection
        .execute_batch("PRAGMA foreign_keys = OFF;")
        .expect("disable foreign keys for corruption injection");
    connection
        .execute(
            "UPDATE commands SET session_id = ?2 WHERE command_id = ?1",
            rusqlite::params![command_id.to_string(), target_session.to_string()],
        )
        .expect("inject cross-session command ownership");
    drop(connection);

    assert!(matches!(
        Kernel::open(&database),
        Err(KernelError::Corrupt(_))
    ));
}

#[tokio::test]
async fn a_semantic_event_cannot_cross_its_operation_session() {
    let directory = tempdir().expect("temporary directory");
    let database = directory.path().join("kernel.sqlite3");
    let kernel = Kernel::open(&database).expect("open kernel");
    let source_session = create_session(&kernel);
    let target_session = create_session(&kernel);
    kernel
        .submit(
            source_session,
            Command::new(CommandId::new(), serde_json::json!("work")),
        )
        .expect("submit command");
    kernel
        .drive(source_session, &event_runtime())
        .await
        .expect("drive command");
    let event_id = kernel
        .events_after(source_session, EventCursor::START)
        .expect("read event")
        .events[0]
        .event_id;
    drop(kernel);

    let connection = rusqlite::Connection::open(&database).expect("open raw database");
    connection
        .execute_batch("PRAGMA foreign_keys = ON;")
        .expect("enable foreign keys");
    assert!(
        connection
            .execute(
                "UPDATE semantic_events SET session_id = ?2 WHERE event_id = ?1",
                rusqlite::params![event_id.to_string(), target_session.to_string()],
            )
            .is_err()
    );
    connection
        .execute_batch("PRAGMA foreign_keys = OFF;")
        .expect("disable foreign keys for corruption injection");
    connection
        .execute(
            "UPDATE semantic_events SET session_id = ?2 WHERE event_id = ?1",
            rusqlite::params![event_id.to_string(), target_session.to_string()],
        )
        .expect("inject cross-session event ownership");
    drop(connection);

    assert!(matches!(
        Kernel::open(&database),
        Err(KernelError::Corrupt(_))
    ));
}

#[test]
fn a_newer_database_schema_fails_closed() {
    let directory = tempdir().expect("temporary directory");
    let database = directory.path().join("kernel.sqlite3");
    let connection = rusqlite::Connection::open(&database).expect("open raw database");
    connection
        .pragma_update(None, "user_version", 3)
        .expect("set future schema");
    drop(connection);

    assert!(matches!(
        Kernel::open(&database),
        Err(KernelError::UnsupportedSchema {
            found: 3,
            supported: 2,
        })
    ));
}

#[test]
fn a_newer_operation_state_fails_when_the_database_opens() {
    let directory = tempdir().expect("temporary directory");
    let database = directory.path().join("kernel.sqlite3");
    let kernel = Kernel::open(&database).expect("open kernel");
    let session_id = create_session(&kernel);
    let admission = kernel
        .submit(
            session_id,
            Command::new(CommandId::new(), serde_json::json!("work")),
        )
        .expect("submit command");
    drop(kernel);
    let connection = rusqlite::Connection::open(&database).expect("open raw database");
    connection
        .execute(
            "UPDATE operations SET state_version = 2 WHERE operation_id = ?1",
            [admission.operation_id.to_string()],
        )
        .expect("set future operation state");
    drop(connection);

    assert!(matches!(
        Kernel::open(&database),
        Err(KernelError::UnsupportedStateVersion {
            found: 2,
            supported: 1,
        })
    ));
}

#[tokio::test]
async fn a_persisted_checkpoint_with_the_wrong_schema_fails_before_the_loop() {
    let directory = tempdir().expect("temporary directory");
    let database = directory.path().join("kernel.sqlite3");
    let kernel = Kernel::open(&database).expect("open kernel");
    let session_id = create_session(&kernel);
    let admission = kernel
        .submit(
            session_id,
            Command::new(CommandId::new(), serde_json::json!("work")),
        )
        .expect("submit command");
    let failing = Runtime::new(
        LoopBinding::new("checkpoint-loop", "1", Arc::new(FailingLoop)),
        1,
        "checkpoint-config-1",
        Vec::new(),
    )
    .expect("valid runtime");
    assert!(matches!(
        kernel.drive(session_id, &failing).await,
        Err(KernelError::Loop(_))
    ));
    drop(kernel);

    let connection = rusqlite::Connection::open(&database).expect("open raw database");
    connection
        .execute(
            "UPDATE operations SET checkpoint_json = ?2 WHERE operation_id = ?1",
            rusqlite::params![
                admission.operation_id.to_string(),
                serde_json::json!({"schema_version": 2, "state": {}}).to_string(),
            ],
        )
        .expect("set incompatible checkpoint");
    drop(connection);

    let kernel = Kernel::open(&database).expect("reopen kernel");
    let called = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let recovery = Runtime::new(
        LoopBinding::new(
            "checkpoint-loop",
            "1",
            Arc::new(NeverCalledLoop(Arc::clone(&called))),
        ),
        1,
        "checkpoint-config-1",
        Vec::new(),
    )
    .expect("valid runtime");
    assert!(matches!(
        kernel.drive(session_id, &recovery).await,
        Err(KernelError::CheckpointSchemaMismatch {
            expected: 1,
            found: 2,
        })
    ));
    assert!(!called.load(std::sync::atomic::Ordering::SeqCst));
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

fn event_runtime() -> Runtime {
    Runtime::new(
        LoopBinding::new("event-loop", "1", Arc::new(EventLoop)),
        1,
        "event-config-1",
        Vec::new(),
    )
    .expect("valid runtime")
}

fn never_called_runtime(called: &Arc<std::sync::atomic::AtomicBool>) -> Runtime {
    Runtime::new(
        LoopBinding::new(
            "never-called-loop",
            "1",
            Arc::new(NeverCalledLoop(Arc::clone(called))),
        ),
        1,
        "never-called-config-1",
        Vec::new(),
    )
    .expect("valid runtime")
}

fn recoverable_effect_runtime(
    plugin: Arc<dyn LoopPlugin>,
    adapter: Arc<dyn EffectAdapter>,
) -> Runtime {
    Runtime::new(
        LoopBinding::new("recoverable-effect-loop", "1", plugin),
        1,
        "recoverable-effect-config-1",
        vec![EffectBinding::new("external", "1", adapter)],
    )
    .expect("valid runtime")
}

struct EventLoop;

impl LoopPlugin for EventLoop {
    fn decide(&self, input: LoopInput) -> Result<LoopDecision, LoopError> {
        Ok(LoopDecision::Complete {
            checkpoint: Checkpoint::new(1, serde_json::json!({"done": true})),
            events: vec![NewEvent::new("value", input.command.content().clone())],
        })
    }
}

struct FailingLoop;

impl LoopPlugin for FailingLoop {
    fn decide(&self, _input: LoopInput) -> Result<LoopDecision, LoopError> {
        Err(LoopError::new("injected loop failure"))
    }
}

struct RequestEffectLoop;

impl LoopPlugin for RequestEffectLoop {
    fn decide(&self, input: LoopInput) -> Result<LoopDecision, LoopError> {
        Ok(LoopDecision::InvokeEffect {
            checkpoint: Checkpoint::new(1, serde_json::json!({"effect": "requested"})),
            binding: "external".to_owned(),
            request: input.command.content().clone(),
            recovery: EffectRecovery::SafeToReplay,
        })
    }
}

struct CrashingEffect;

impl EffectAdapter for CrashingEffect {
    fn invoke(&self, _invocation: EffectInvocation) -> EffectFuture<'_> {
        panic!("injected process loss after effect dispatch")
    }
}

struct NeverCalledEffect(Arc<std::sync::atomic::AtomicBool>);

impl EffectAdapter for NeverCalledEffect {
    fn invoke(&self, _invocation: EffectInvocation) -> EffectFuture<'_> {
        self.0.store(true, std::sync::atomic::Ordering::SeqCst);
        Box::pin(std::future::ready(
            EffectOutcome::Failure {
                message: "effect must not run".to_owned(),
            }
            .into(),
        ))
    }
}

struct NeverCalledLoop(Arc<std::sync::atomic::AtomicBool>);

impl LoopPlugin for NeverCalledLoop {
    fn decide(&self, _input: LoopInput) -> Result<LoopDecision, LoopError> {
        self.0.store(true, std::sync::atomic::Ordering::SeqCst);
        panic!("loop must not run")
    }
}
