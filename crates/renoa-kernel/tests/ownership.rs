use std::{
    env,
    error::Error as _,
    io::{BufRead, BufReader, Read},
    process::{Command as ProcessCommand, Stdio},
    sync::Arc,
};

use renoa_kernel::{
    AgentId, Checkpoint, Command, CommandId, EffectAdapter, EffectBinding, EffectFuture,
    EffectInvocation, EffectOutcome, EffectRecovery, EffectStatus, Kernel, KernelError,
    LoopBinding, LoopDecision, LoopError, LoopInput, LoopPlugin, OperationStatus, Runtime,
    SessionId, StoreErrorKind,
};
use tempfile::tempdir;
use tokio::sync::Notify;

#[test]
fn only_one_kernel_process_owner_can_open_a_database() {
    let directory = tempdir().expect("temporary directory");
    let database = directory.path().join("kernel.sqlite3");
    let _owner = Kernel::open(&database).expect("open owner");

    assert!(matches!(
        Kernel::open(&database),
        Err(KernelError::AlreadyRunning { .. })
    ));
}

#[test]
fn storage_errors_preserve_their_kind_and_source() {
    let directory = tempdir().expect("temporary directory");
    let missing_parent = directory.path().join("missing").join("kernel.sqlite3");

    let Err(KernelError::Store(error)) = Kernel::open(&missing_parent) else {
        panic!("opening a missing parent returned the wrong error kind");
    };
    assert_eq!(error.kind(), StoreErrorKind::Io);
    assert!(error.source().is_some(), "I/O source was discarded");
}

#[test]
fn a_second_process_cannot_open_the_live_database() {
    let directory = tempdir().expect("temporary directory");
    let database = directory.path().join("kernel.sqlite3");
    let mut child = ProcessCommand::new(env::current_exe().expect("current test binary"))
        .args(["--exact", "lock_holder_process", "--ignored", "--nocapture"])
        .env("RENOA_KERNEL_LOCK_TEST_DB", &database)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .expect("spawn lock holder");
    let mut output = BufReader::new(child.stdout.take().expect("child stdout"));
    let mut line = String::new();
    loop {
        let bytes = output.read_line(&mut line).expect("read ready signal");
        assert_ne!(bytes, 0, "lock holder exited before becoming ready");
        if line.trim() == "READY" {
            break;
        }
        line.clear();
    }

    assert!(matches!(
        Kernel::open(&database),
        Err(KernelError::AlreadyRunning { .. })
    ));
    drop(child.stdin.take());
    assert!(child.wait().expect("wait for lock holder").success());
    Kernel::open(&database).expect("lock released after owner exits");
}

#[test]
#[ignore = "helper process for a_second_process_cannot_open_the_live_database"]
fn lock_holder_process() {
    let database = env::var_os("RENOA_KERNEL_LOCK_TEST_DB").expect("lock-test database path");
    let _kernel = Kernel::open(database).expect("lock database");
    println!("READY");
    std::io::stdin()
        .read_to_end(&mut Vec::new())
        .expect("wait for parent");
}

#[tokio::test]
async fn only_one_live_driver_can_own_a_session() {
    let directory = tempdir().expect("temporary directory");
    let kernel =
        Arc::new(Kernel::open(directory.path().join("kernel.sqlite3")).expect("open kernel"));
    let agent_id = AgentId::new();
    let session_id = SessionId::new();
    kernel.create_agent(agent_id).expect("create agent");
    kernel
        .create_session(session_id, agent_id)
        .expect("create session");
    kernel
        .submit(
            session_id,
            Command::new(CommandId::new(), serde_json::json!({"wait": true})),
        )
        .expect("submit command");
    let invoked = Arc::new(Notify::new());
    let release = Arc::new(Notify::new());
    let runtime = pending_runtime(Arc::clone(&invoked), Arc::clone(&release));
    let runner = Arc::clone(&kernel);
    let task = tokio::spawn(async move { runner.drive(session_id, &runtime).await });
    invoked.notified().await;

    let competing = pending_runtime(Arc::new(Notify::new()), Arc::new(Notify::new()));
    assert!(matches!(
        kernel.drive(session_id, &competing).await,
        Err(KernelError::Busy(id)) if id == session_id
    ));

    release.notify_one();
    task.await
        .expect("driver task")
        .expect("finish first driver");
}

#[tokio::test]
async fn dropped_driver_holds_the_session_until_effect_cleanup_finishes() {
    let directory = tempdir().expect("temporary directory");
    let database = directory.path().join("kernel.sqlite3");
    let kernel = Arc::new(Kernel::open(&database).expect("open kernel"));
    let agent_id = AgentId::new();
    let session_id = SessionId::new();
    kernel.create_agent(agent_id).expect("create agent");
    kernel
        .create_session(session_id, agent_id)
        .expect("create session");
    kernel
        .submit(
            session_id,
            Command::new(CommandId::new(), serde_json::json!({"cancel": true})),
        )
        .expect("submit command");
    let invoked = Arc::new(Notify::new());
    let cleanup_started = Arc::new(Notify::new());
    let release_cleanup = Arc::new(Notify::new());
    let runtime = cancellation_runtime(
        Arc::clone(&invoked),
        Arc::clone(&cleanup_started),
        Arc::clone(&release_cleanup),
    );
    let runner = Arc::clone(&kernel);
    let task = tokio::spawn(async move { runner.drive(session_id, &runtime).await });
    invoked.notified().await;

    task.abort();
    assert!(task.await.expect_err("driver was aborted").is_cancelled());
    cleanup_started.notified().await;

    let competing = cancellation_runtime(
        Arc::new(Notify::new()),
        Arc::new(Notify::new()),
        Arc::new(Notify::new()),
    );
    assert!(matches!(
        kernel.drive(session_id, &competing).await,
        Err(KernelError::Busy(id)) if id == session_id
    ));
    drop(kernel);
    assert!(matches!(
        Kernel::open(&database),
        Err(KernelError::AlreadyRunning { .. })
    ));

    release_cleanup.notify_one();
    let kernel = loop {
        match Kernel::open(&database) {
            Err(KernelError::AlreadyRunning { .. }) => tokio::task::yield_now().await,
            result => break result.expect("reopen after cleanup"),
        }
    };
    let resumed = kernel
        .drive(session_id, &competing)
        .await
        .expect("resume after cleanup");
    assert!(matches!(resumed, renoa_kernel::DriveResult::Blocked { .. }));
    let blocked = kernel
        .inspect(session_id)
        .expect("inspect blocked operation");
    assert_eq!(
        blocked.operations[0].status,
        OperationStatus::OutcomeUnknown
    );
    assert_eq!(
        blocked.operations[0].effects[0].status,
        EffectStatus::OutcomeUnknown
    );
}

fn pending_runtime(invoked: Arc<Notify>, release: Arc<Notify>) -> Runtime {
    Runtime::new(
        LoopBinding::new("pending-loop", "1", Arc::new(PendingLoop)),
        1,
        "pending-config-1",
        vec![EffectBinding::new(
            "pending",
            "1",
            Arc::new(PendingAdapter { invoked, release }),
        )],
    )
    .expect("valid runtime")
}

fn cancellation_runtime(
    invoked: Arc<Notify>,
    cleanup_started: Arc<Notify>,
    release_cleanup: Arc<Notify>,
) -> Runtime {
    Runtime::new(
        LoopBinding::new("cancellation-loop", "1", Arc::new(PendingLoop)),
        1,
        "cancellation-config-1",
        vec![EffectBinding::new(
            "pending",
            "1",
            Arc::new(CancellationAdapter {
                invoked,
                cleanup_started,
                release_cleanup,
            }),
        )],
    )
    .expect("valid runtime")
}

struct PendingLoop;

impl LoopPlugin for PendingLoop {
    fn decide(&self, input: LoopInput) -> Result<LoopDecision, LoopError> {
        if input.effect.is_none() {
            Ok(LoopDecision::InvokeEffect {
                checkpoint: Checkpoint::new(1, serde_json::json!({"waiting": true})),
                binding: "pending".to_owned(),
                request: input.command.content().clone(),
                recovery: EffectRecovery::NeverReplay,
            })
        } else {
            Ok(LoopDecision::Complete {
                checkpoint: Checkpoint::new(1, serde_json::json!({"done": true})),
                events: Vec::new(),
            })
        }
    }
}

struct PendingAdapter {
    invoked: Arc<Notify>,
    release: Arc<Notify>,
}

impl EffectAdapter for PendingAdapter {
    fn invoke(&self, _invocation: EffectInvocation) -> EffectFuture<'_> {
        Box::pin(async move {
            self.invoked.notify_one();
            self.release.notified().await;
            EffectOutcome::Success(serde_json::json!({"released": true})).into()
        })
    }
}

struct CancellationAdapter {
    invoked: Arc<Notify>,
    cleanup_started: Arc<Notify>,
    release_cleanup: Arc<Notify>,
}

impl EffectAdapter for CancellationAdapter {
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
