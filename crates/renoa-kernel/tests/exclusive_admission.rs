use std::sync::{Arc, Barrier};

use renoa_kernel::{AgentId, Command, CommandId, Kernel, KernelError, SessionId};
use serde_json::json;
use tempfile::tempdir;

#[test]
fn exclusive_admission_is_atomic_under_concurrent_callers() {
    let directory = tempdir().expect("temporary directory");
    let kernel = Arc::new(
        Kernel::open(directory.path().join("kernel.sqlite3")).expect("open kernel database"),
    );
    let agent_id = AgentId::new();
    let session_id = SessionId::new();
    kernel.create_agent(agent_id).expect("create agent");
    kernel
        .create_session(session_id, agent_id)
        .expect("create session");
    let barrier = Arc::new(Barrier::new(3));

    let first = spawn_admission(
        Arc::clone(&kernel),
        Arc::clone(&barrier),
        session_id,
        Command::new(CommandId::new(), json!({"turn": 1})),
    );
    let second = spawn_admission(
        Arc::clone(&kernel),
        Arc::clone(&barrier),
        session_id,
        Command::new(CommandId::new(), json!({"turn": 2})),
    );
    barrier.wait();
    let results = [
        first.join().expect("join first admission"),
        second.join().expect("join second admission"),
    ];

    assert_eq!(results.iter().filter(|result| result.is_ok()).count(), 1);
    assert_eq!(
        results
            .iter()
            .filter(|result| matches!(result, Err(KernelError::UnfinishedOperation { .. })))
            .count(),
        1
    );
    assert_eq!(
        kernel
            .inspect(session_id)
            .expect("inspect session")
            .operations
            .len(),
        1,
        "a rejected concurrent command must not leave a queued operation"
    );
}

fn spawn_admission(
    kernel: Arc<Kernel>,
    barrier: Arc<Barrier>,
    session_id: SessionId,
    command: Command,
) -> std::thread::JoinHandle<Result<renoa_kernel::Admission, KernelError>> {
    std::thread::spawn(move || {
        barrier.wait();
        kernel.submit_exclusive(session_id, command)
    })
}
