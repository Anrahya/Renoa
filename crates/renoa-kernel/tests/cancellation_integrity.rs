use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};

use renoa_kernel::{
    AgentId, CancellationId, CancellationInput, CancellationTransition, Checkpoint, Command,
    CommandId, Kernel, KernelError, LoopBinding, LoopDecision, LoopError, LoopInput, LoopPlugin,
    NewEvent, Runtime, SessionId,
};
use tempfile::tempdir;

#[tokio::test]
async fn corrupted_history_stops_cancellation_before_the_loop_or_any_effect() {
    let directory = tempdir().expect("temporary directory");
    let database = directory.path().join("kernel.sqlite3");
    let kernel = Kernel::open(&database).expect("open kernel");
    let agent_id = AgentId::new();
    let session_id = SessionId::new();
    kernel.create_agent(agent_id).expect("create agent");
    kernel
        .create_session(session_id, agent_id)
        .expect("create session");
    let admission = kernel
        .submit(
            session_id,
            Command::new(CommandId::new(), serde_json::json!({"work": true})),
        )
        .expect("submit command");
    assert!(matches!(
        kernel.request_cancellation(
            session_id,
            admission.operation_id,
            CancellationId::new()
        ),
        Err(KernelError::OperationNotCancellable(id)) if id == admission.operation_id
    ));

    let cancellation_calls = Arc::new(AtomicUsize::new(0));
    let runtime = Runtime::new(
        LoopBinding::new(
            "gap-loop",
            "1",
            Arc::new(GapLoop {
                cancellation_calls: Arc::clone(&cancellation_calls),
            }),
        ),
        1,
        "gap-config-1",
        Vec::new(),
    )
    .expect("valid runtime");
    assert!(matches!(
        kernel.drive(session_id, &runtime).await,
        Err(KernelError::Loop(error)) if error.message() == "pause after durable event"
    ));
    kernel
        .request_cancellation(session_id, admission.operation_id, CancellationId::new())
        .expect("persist cancellation");
    let connection = rusqlite::Connection::open(&database).expect("open corruption connection");
    connection
        .execute("DELETE FROM semantic_events", [])
        .expect("remove semantic event");
    drop(connection);

    assert!(matches!(
        kernel.drive(session_id, &runtime).await,
        Err(KernelError::Corrupt(message)) if message.contains("below high-water mark")
    ));
    assert_eq!(cancellation_calls.load(Ordering::SeqCst), 0);
}

struct GapLoop {
    cancellation_calls: Arc<AtomicUsize>,
}

impl LoopPlugin for GapLoop {
    fn decide(&self, input: LoopInput) -> Result<LoopDecision, LoopError> {
        if input.checkpoint.is_none() {
            Ok(LoopDecision::AppendEventsAndContinue {
                checkpoint: Checkpoint::new(1, serde_json::json!({"event": true})),
                events: vec![NewEvent::new("proof", serde_json::json!(true))],
            })
        } else {
            Err(LoopError::new("pause after durable event"))
        }
    }

    fn cancel_operation(
        &self,
        _input: CancellationInput,
    ) -> Result<CancellationTransition, LoopError> {
        self.cancellation_calls.fetch_add(1, Ordering::SeqCst);
        Ok(CancellationTransition {
            checkpoint: Checkpoint::new(1, serde_json::json!({"cancelled": true})),
            events: Vec::new(),
        })
    }
}
