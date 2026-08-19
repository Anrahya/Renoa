use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};

use renoa_kernel::{
    AgentId, Command, CommandId, Kernel, KernelError, LoopBinding, LoopDecision, LoopError,
    LoopInput, LoopPlugin, Runtime, SessionId,
};
use tempfile::tempdir;

#[tokio::test]
async fn corrupted_command_ownership_stops_the_live_drive_before_the_loop() {
    let directory = tempdir().expect("temporary directory");
    let database = directory.path().join("kernel.sqlite3");
    let kernel = Kernel::open(&database).expect("open kernel");
    let source_session = create_session(&kernel);
    let target_session = create_session(&kernel);
    let command_id = CommandId::new();
    kernel
        .submit(
            source_session,
            Command::new(command_id, serde_json::json!({"work": true})),
        )
        .expect("submit command");

    let connection = rusqlite::Connection::open(&database).expect("open corruption connection");
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

    let called = Arc::new(AtomicBool::new(false));
    let runtime = Runtime::new(
        LoopBinding::new(
            "never-called-loop",
            "1",
            Arc::new(NeverCalledLoop(Arc::clone(&called))),
        ),
        1,
        "never-called-config-1",
        Vec::new(),
    )
    .expect("valid runtime");
    assert!(matches!(
        kernel.drive(source_session, &runtime).await,
        Err(KernelError::Corrupt(_))
    ));
    assert!(!called.load(Ordering::SeqCst));
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

struct NeverCalledLoop(Arc<AtomicBool>);

impl LoopPlugin for NeverCalledLoop {
    fn decide(&self, _input: LoopInput) -> Result<LoopDecision, LoopError> {
        self.0.store(true, Ordering::SeqCst);
        panic!("corrupted command must not reach the loop")
    }
}
