use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};

use tempfile::tempdir;
use tokio::sync::Notify;

use super::completing_runtime;
use crate::{
    AgentId, Checkpoint, Command, CommandId, DriveResult, EffectAdapter, EffectBinding,
    EffectFuture, EffectInvocation, EffectOutcome, EffectRecovery, EffectRequest, Kernel,
    KernelError, LoopBinding, LoopDecision, LoopError, LoopInput, LoopPlugin, NewEvent,
    OperationOutcome, Runtime, SessionId, SteerState, schema::open_connection,
};

fn session(kernel: &Kernel) -> SessionId {
    let agent_id = AgentId::new();
    let session_id = SessionId::new();
    kernel.create_agent(agent_id).expect("create agent");
    kernel
        .create_session(session_id, agent_id)
        .expect("create session");
    session_id
}

fn command(text: &str) -> Command {
    Command::new(CommandId::new(), serde_json::json!({ "text": text }))
}

/// Runs one effect and records that it settled, then completes. Pending
/// steers are accepted at any decision with no settled effect waiting, so a
/// steer recorded while the effect runs reaches the decision after it.
struct SteeredLoop;

impl LoopPlugin for SteeredLoop {
    fn decide(&self, input: LoopInput) -> Result<LoopDecision, LoopError> {
        let step = input
            .checkpoint
            .as_ref()
            .and_then(|checkpoint| checkpoint.state()["step"].as_u64())
            .unwrap_or(0);
        if input.effect_batch.is_some() {
            return Ok(LoopDecision::AppendEventsAndContinue {
                checkpoint: Checkpoint::new(1, serde_json::json!({ "step": 2 })),
                events: vec![NewEvent::new("settled", serde_json::json!(true))],
            });
        }
        if !input.steers.is_empty() {
            return Ok(LoopDecision::AcceptSteers {
                checkpoint: Checkpoint::new(1, serde_json::json!({ "step": step })),
                events: input
                    .steers
                    .iter()
                    .map(|steer| NewEvent::new("steer", steer.content().clone()))
                    .collect(),
                steers: input.steers.iter().map(Command::command_id).collect(),
            });
        }
        if step == 0 {
            return Ok(LoopDecision::InvokeEffects {
                checkpoint: Checkpoint::new(1, serde_json::json!({ "step": 1 })),
                effects: vec![EffectRequest {
                    binding: "work".to_owned(),
                    request: serde_json::json!({}),
                    recovery: EffectRecovery::SafeToReplay,
                }],
            });
        }
        Ok(LoopDecision::Complete {
            checkpoint: Checkpoint::new(1, serde_json::json!({ "step": 3 })),
            events: vec![NewEvent::new("done", serde_json::json!(true))],
        })
    }
}

/// Signals `started`, then waits for `release` before settling.
struct GatedAdapter {
    started: Arc<Notify>,
    release: Arc<Notify>,
}

impl EffectAdapter for GatedAdapter {
    fn invoke(&self, _invocation: EffectInvocation) -> EffectFuture<'_> {
        Box::pin(async move {
            self.started.notify_one();
            self.release.notified().await;
            EffectOutcome::Success(serde_json::json!({})).into()
        })
    }
}

fn steered_runtime(adapter: Arc<dyn EffectAdapter>) -> Runtime {
    Runtime::new(
        LoopBinding::new("steered-loop", "1", Arc::new(SteeredLoop)),
        1,
        "steered-config-1",
        vec![EffectBinding::new("work", "1", adapter)],
    )
    .expect("valid runtime")
}

fn event_kinds(kernel: &Kernel, session_id: SessionId) -> Vec<(String, serde_json::Value)> {
    kernel
        .events_after(session_id, crate::EventCursor::START)
        .expect("read events")
        .events
        .into_iter()
        .map(|event| (event.kind, event.payload))
        .collect()
}

#[tokio::test]
async fn a_steer_recorded_during_an_effect_reaches_the_next_decision() {
    let directory = tempdir().expect("temporary directory");
    let kernel = Kernel::open(directory.path().join("kernel.sqlite3")).expect("open kernel");
    let session_id = session(&kernel);
    let target = command("make a video");
    let target_id = target.command_id();
    kernel
        .submit_exclusive(session_id, target)
        .expect("submit target");
    let started = Arc::new(Notify::new());
    let release = Arc::new(Notify::new());
    let runtime = steered_runtime(Arc::new(GatedAdapter {
        started: Arc::clone(&started),
        release: Arc::clone(&release),
    }));
    let steer = command("make it shorter");
    let steer_id = steer.command_id();

    let (result, ()) = tokio::join!(kernel.drive(session_id, &runtime), async {
        started.notified().await;
        assert_eq!(
            kernel
                .request_steer(session_id, target_id, steer.clone())
                .expect("record steer"),
            SteerState::Pending
        );
        release.notify_one();
    });

    assert!(matches!(
        result.expect("drive"),
        DriveResult::Finished {
            outcome: OperationOutcome::Completed,
            ..
        }
    ));
    let kinds = event_kinds(&kernel, session_id);
    assert_eq!(
        kinds,
        vec![
            ("settled".to_owned(), serde_json::json!(true)),
            (
                "steer".to_owned(),
                serde_json::json!({ "text": "make it shorter" })
            ),
            ("done".to_owned(), serde_json::json!(true)),
        ]
    );
    assert_eq!(
        kernel
            .request_steer(session_id, target_id, steer.clone())
            .expect("retry the steer"),
        SteerState::Accepted
    );
    assert!(matches!(
        kernel.submit_exclusive(session_id, steer),
        Err(KernelError::SteerAccepted { command_id, target })
            if command_id == steer_id && target == target_id
    ));
    assert_eq!(
        kernel
            .inspect(session_id)
            .expect("inspect")
            .operations
            .len(),
        1
    );
}

#[tokio::test]
async fn a_steer_recorded_before_its_target_starts_waits_for_it() {
    let directory = tempdir().expect("temporary directory");
    let kernel = Kernel::open(directory.path().join("kernel.sqlite3")).expect("open kernel");
    let session_id = session(&kernel);
    let target = command("first");
    let steer = command("and then this");
    assert_eq!(
        kernel
            .request_steer(session_id, target.command_id(), steer.clone())
            .expect("record steer early"),
        SteerState::Pending
    );
    kernel
        .submit_exclusive(session_id, target.clone())
        .expect("submit target");
    let started = Arc::new(Notify::new());
    let release = Arc::new(Notify::new());
    release.notify_one();
    kernel
        .drive(
            session_id,
            &steered_runtime(Arc::new(GatedAdapter { started, release })),
        )
        .await
        .expect("drive");

    assert_eq!(event_kinds(&kernel, session_id)[0].0, "steer");
    assert_eq!(
        kernel
            .request_steer(session_id, target.command_id(), steer)
            .expect("retry"),
        SteerState::Accepted
    );
}

#[tokio::test]
async fn a_steer_its_operation_never_accepted_runs_as_its_own_command() {
    let directory = tempdir().expect("temporary directory");
    let kernel = Kernel::open(directory.path().join("kernel.sqlite3")).expect("open kernel");
    let session_id = session(&kernel);
    let target = command("first");
    kernel
        .submit_exclusive(session_id, target.clone())
        .expect("submit target");
    let steer = command("too late");
    kernel
        .request_steer(session_id, target.command_id(), steer.clone())
        .expect("record steer");
    let decisions = Arc::new(AtomicUsize::new(0));
    kernel
        .drive(session_id, &completing_runtime(Arc::clone(&decisions)))
        .await
        .expect("drive a loop that accepts no steer");

    assert_eq!(
        kernel
            .request_steer(session_id, target.command_id(), steer.clone())
            .expect("retry"),
        SteerState::Released
    );
    kernel
        .submit_exclusive(session_id, steer)
        .expect("a released steer is an ordinary command");
    kernel
        .drive(session_id, &completing_runtime(Arc::clone(&decisions)))
        .await
        .expect("drive the released steer");
    assert_eq!(decisions.load(Ordering::SeqCst), 2);
}

#[tokio::test]
async fn a_refused_steer_leaves_nothing_behind() {
    let directory = tempdir().expect("temporary directory");
    let database = directory.path().join("kernel.sqlite3");
    let kernel = Kernel::open(&database).expect("open kernel");
    let session_id = session(&kernel);
    let other_session = session(&kernel);
    let finished = command("finished");
    kernel
        .submit_exclusive(session_id, finished.clone())
        .expect("submit");
    kernel
        .drive(
            session_id,
            &completing_runtime(Arc::new(AtomicUsize::new(0))),
        )
        .await
        .expect("finish it");
    let running = command("running");
    kernel
        .submit_exclusive(session_id, running.clone())
        .expect("submit running");
    let recorded = command("recorded");
    kernel
        .request_steer(session_id, running.command_id(), recorded.clone())
        .expect("record one steer");

    assert!(matches!(
        kernel.request_steer(session_id, finished.command_id(), command("late")),
        Err(KernelError::SteerTargetUnavailable(_))
    ));
    assert!(matches!(
        kernel.request_steer(other_session, running.command_id(), command("elsewhere")),
        Err(KernelError::SteerTargetUnavailable(_))
    ));
    assert!(matches!(
        kernel.request_steer(session_id, recorded.command_id(), command("of a steer")),
        Err(KernelError::SteerTargetUnavailable(_))
    ));
    assert!(matches!(
        kernel.request_steer(session_id, running.command_id(), finished.clone()),
        Err(KernelError::CommandConflict { .. })
    ));
    assert!(matches!(
        kernel.request_steer(session_id, finished.command_id(), recorded.clone()),
        Err(KernelError::SteerConflict { .. })
    ));
    let changed = Command::new(
        recorded.command_id(),
        serde_json::json!({ "text": "changed" }),
    );
    assert!(matches!(
        kernel.request_steer(session_id, running.command_id(), changed),
        Err(KernelError::SteerConflict { .. })
    ));
    assert!(matches!(
        kernel.request_steer(SessionId::new(), running.command_id(), command("nowhere")),
        Err(KernelError::SessionNotFound(_))
    ));

    let stored: Vec<String> = open_connection(&database)
        .expect("open database")
        .prepare("SELECT content_json FROM steer_requests")
        .expect("prepare")
        .query_map([], |row| row.get(0))
        .expect("query")
        .collect::<Result<_, _>>()
        .expect("rows");
    assert_eq!(
        stored,
        vec![serde_json::to_string(&recorded).expect("encode")]
    );
}

/// Accepts a steer that was never recorded.
struct ForgingLoop;

impl LoopPlugin for ForgingLoop {
    fn decide(&self, _input: LoopInput) -> Result<LoopDecision, LoopError> {
        Ok(LoopDecision::AcceptSteers {
            checkpoint: Checkpoint::new(1, serde_json::json!({})),
            events: vec![NewEvent::new("steer", serde_json::json!({}))],
            steers: vec![CommandId::new()],
        })
    }
}

#[tokio::test]
async fn accepting_a_steer_that_is_not_pending_commits_nothing() {
    let directory = tempdir().expect("temporary directory");
    let kernel = Kernel::open(directory.path().join("kernel.sqlite3")).expect("open kernel");
    let session_id = session(&kernel);
    kernel
        .submit_exclusive(session_id, command("target"))
        .expect("submit");
    let runtime = Runtime::new(
        LoopBinding::new("forging-loop", "1", Arc::new(ForgingLoop)),
        1,
        "forging-config-1",
        Vec::new(),
    )
    .expect("valid runtime");

    assert!(matches!(
        kernel.drive(session_id, &runtime).await,
        Err(KernelError::InvalidDecision(_))
    ));
    assert!(event_kinds(&kernel, session_id).is_empty());
}

#[test]
fn schema_four_gains_the_table_of_steers() {
    let directory = tempdir().expect("temporary directory");
    let database = directory.path().join("kernel.sqlite3");
    let kernel = Kernel::open(&database).expect("create kernel");
    let session_id = session(&kernel);
    drop(kernel);
    open_connection(&database)
        .expect("open database")
        .execute_batch(
            "DROP TABLE steer_requests;
             PRAGMA user_version = 4;",
        )
        .expect("represent schema four");

    let kernel = Kernel::open(&database).expect("upgrade schema four");
    let target = command("target");
    kernel
        .submit_exclusive(session_id, target.clone())
        .expect("submit");
    assert_eq!(
        kernel
            .request_steer(session_id, target.command_id(), command("steer"))
            .expect("record a steer after the upgrade"),
        SteerState::Pending
    );
    drop(kernel);
    let version: i64 = open_connection(&database)
        .expect("reopen")
        .query_row("PRAGMA user_version", [], |row| row.get(0))
        .expect("version");
    assert_eq!(version, 5);
}

/// Runs one effect, then tries to accept steers instead of deciding on it.
struct SkippingLoop;

impl LoopPlugin for SkippingLoop {
    fn decide(&self, input: LoopInput) -> Result<LoopDecision, LoopError> {
        if input.effect_batch.is_none() {
            return Ok(LoopDecision::InvokeEffects {
                checkpoint: Checkpoint::new(1, serde_json::json!({ "step": 1 })),
                effects: vec![EffectRequest {
                    binding: "work".to_owned(),
                    request: serde_json::json!({}),
                    recovery: EffectRecovery::SafeToReplay,
                }],
            });
        }
        Ok(LoopDecision::AcceptSteers {
            checkpoint: Checkpoint::new(1, serde_json::json!({ "step": 1 })),
            events: vec![NewEvent::new("steer", serde_json::json!({}))],
            steers: input.steers.iter().map(Command::command_id).collect(),
        })
    }
}

#[tokio::test]
async fn a_settled_effect_is_decided_on_before_any_steer_is_accepted() {
    let directory = tempdir().expect("temporary directory");
    let kernel = Kernel::open(directory.path().join("kernel.sqlite3")).expect("open kernel");
    let session_id = session(&kernel);
    let target = command("target");
    kernel
        .submit_exclusive(session_id, target.clone())
        .expect("submit");
    let steer = command("steer");
    kernel
        .request_steer(session_id, target.command_id(), steer.clone())
        .expect("record steer");
    let release = Arc::new(Notify::new());
    release.notify_one();
    let runtime = Runtime::new(
        LoopBinding::new("skipping-loop", "1", Arc::new(SkippingLoop)),
        1,
        "skipping-config-1",
        vec![EffectBinding::new(
            "work",
            "1",
            Arc::new(GatedAdapter {
                started: Arc::new(Notify::new()),
                release,
            }),
        )],
    )
    .expect("valid runtime");

    // Without the refusal the loop would accept, rerun its effect and repeat.
    let driven = tokio::time::timeout(
        std::time::Duration::from_secs(5),
        kernel.drive(session_id, &runtime),
    )
    .await
    .expect("the refusal ends the drive");
    assert!(matches!(driven, Err(KernelError::InvalidDecision(_))));
    assert_eq!(
        kernel.steer_state(steer.command_id()).expect("state"),
        Some(SteerState::Pending)
    );
}
