use std::sync::{Arc, Mutex, Weak};

use tempfile::tempdir;

use super::{
    RecordingAdapter, UnknownAdapter, batch_recovery_runtime, effect_runtime, kernel_with_command,
};
use crate::{
    CancellationId, CancellationInput, CancellationTransition, Checkpoint, Command, CommandId,
    DriveResult, EffectAdapter, EffectBinding, EffectFuture, EffectInvocation, EffectOutcome,
    EffectRecovery, EffectRequest, EffectStatus, Kernel, KernelError, LoopBinding, LoopDecision,
    LoopError, LoopInput, LoopPlugin, OperationOutcome, OperationStatus, Runtime, SessionId,
    UnknownEffectAbandonment, UnknownEffectInput, schema::open_connection,
};

/// Every stored request byte, however it is shaped.
fn stored_request_bytes(database: &std::path::Path) -> i64 {
    open_connection(database)
        .expect("open kernel database")
        .query_row(
            "SELECT coalesce(sum(length(CAST(request_json AS BLOB))), 0) FROM effects",
            [],
            |row| row.get(0),
        )
        .expect("sum stored requests")
}

#[tokio::test]
async fn a_finished_operation_keeps_its_outcome_but_no_request() {
    let directory = tempdir().expect("temporary directory");
    let database = directory.path().join("kernel.sqlite3");
    let (kernel, session_id) = kernel_with_command(&database);
    let calls = Arc::new(Mutex::new(Vec::new()));
    let runtime = effect_runtime(
        EffectRecovery::SafeToReplay,
        Arc::new(RecordingAdapter(Arc::clone(&calls))),
    );
    let context = "x".repeat(64 * 1024);
    for turn in 0..4 {
        if turn > 0 {
            kernel
                .submit(
                    session_id,
                    Command::new(
                        CommandId::new(),
                        serde_json::json!({ "context": context, "turn": turn }),
                    ),
                )
                .expect("submit another turn");
        }
        assert!(matches!(
            kernel
                .drive(session_id, &runtime)
                .await
                .expect("drive turn"),
            DriveResult::Finished {
                outcome: OperationOutcome::Completed,
                ..
            }
        ));
    }
    assert_eq!(calls.lock().expect("calls lock").len(), 4);
    assert_eq!(
        stored_request_bytes(&database),
        0,
        "stored requests do not grow with calls times context"
    );
    let snapshot = kernel.inspect(session_id).expect("inspect session");
    for operation in &snapshot.operations {
        let effect = &operation.effect_batches[0].effects[0];
        assert_eq!(effect.binding, "external");
        assert_eq!(effect.status, EffectStatus::Settled);
        assert!(effect.outcome.is_some());
        assert_eq!(effect.request, None);
    }
}

/// Calls one effect, then ends the operation waiting for input.
struct WaitingLoop;

impl LoopPlugin for WaitingLoop {
    fn decide(&self, input: LoopInput) -> Result<LoopDecision, LoopError> {
        let checkpoint = Checkpoint::new(1, serde_json::json!({}));
        if input.effect_batch.is_some() {
            return Ok(LoopDecision::WaitForInput {
                checkpoint,
                events: Vec::new(),
            });
        }
        Ok(LoopDecision::InvokeEffects {
            checkpoint,
            effects: vec![EffectRequest {
                binding: "external".to_owned(),
                request: input.command.content().clone(),
                recovery: EffectRecovery::SafeToReplay,
            }],
        })
    }
}

#[tokio::test]
async fn an_operation_waiting_for_input_releases_its_requests() {
    let directory = tempdir().expect("temporary directory");
    let database = directory.path().join("kernel.sqlite3");
    let (kernel, session_id) = kernel_with_command(&database);
    let runtime = Runtime::new(
        LoopBinding::new("waiting-loop", "1", Arc::new(WaitingLoop)),
        1,
        "waiting-config-1",
        vec![EffectBinding::new(
            "external",
            "1",
            Arc::new(RecordingAdapter(Arc::new(Mutex::new(Vec::new())))),
        )],
    )
    .expect("valid runtime");
    assert!(matches!(
        kernel.drive(session_id, &runtime).await.expect("drive"),
        DriveResult::Finished {
            outcome: OperationOutcome::WaitingForInput,
            ..
        }
    ));
    assert_eq!(stored_request_bytes(&database), 0);
}

#[tokio::test]
async fn a_batch_with_an_unknown_child_keeps_every_request_through_abandonment() {
    let directory = tempdir().expect("temporary directory");
    let database = directory.path().join("kernel.sqlite3");
    let (kernel, session_id) = kernel_with_command(&database);
    let runtime = batch_recovery_runtime(
        Arc::new(RecordingAdapter(Arc::new(Mutex::new(Vec::new())))),
        Arc::new(UnknownAdapter),
    );
    let DriveResult::Blocked { operation_id } = kernel
        .drive(session_id, &runtime)
        .await
        .expect("drive batch")
    else {
        panic!("an unknown child blocks the operation")
    };
    let abandoned = kernel
        .abandon_unknown_effect(session_id, operation_id, &runtime)
        .expect("abandon the unknown child");

    let snapshot = kernel.inspect(session_id).expect("inspect abandoned batch");
    let effects = &snapshot.operations[0].effect_batches[0].effects;
    assert_eq!(effects[0].status, EffectStatus::Settled);
    assert_eq!(effects[1].status, EffectStatus::OutcomeUnknown);
    assert!(
        effects.iter().all(|effect| effect.request.is_some()),
        "an unknown outcome keeps its batch's requests to be examined"
    );
    assert_eq!(
        kernel
            .abandon_unknown_effect(session_id, operation_id, &runtime)
            .expect("a retried abandonment still reads the batch"),
        abandoned
    );
}

#[tokio::test]
async fn schema_three_releases_finished_requests_and_reclaims_their_space() {
    let directory = tempdir().expect("temporary directory");
    let database = directory.path().join("kernel.sqlite3");
    let (kernel, session_id) = kernel_with_command(&database);
    let finished = effect_runtime(
        EffectRecovery::SafeToReplay,
        Arc::new(RecordingAdapter(Arc::new(Mutex::new(Vec::new())))),
    );
    kernel.drive(session_id, &finished).await.expect("finish");
    kernel
        .submit(
            session_id,
            Command::new(CommandId::new(), serde_json::json!({"work": "pending"})),
        )
        .expect("submit unfinished work");
    let blocked = effect_runtime(EffectRecovery::NeverReplay, Arc::new(UnknownAdapter));
    assert!(matches!(
        kernel.drive(session_id, &blocked).await.expect("block"),
        DriveResult::Blocked { .. }
    ));
    drop(kernel);

    // Schema 3 kept every request and had no release.
    let connection = open_connection(&database).expect("open fixture database");
    connection
        .execute_batch(
            "PRAGMA foreign_keys = OFF;
             DROP TRIGGER release_finished_effect_requests;
             DROP VIEW finished_effect_requests;
             ALTER TABLE effects RENAME TO effects_v4;
             CREATE TABLE effects (
                effect_id TEXT PRIMARY KEY NOT NULL,
                batch_id TEXT NOT NULL REFERENCES effect_batches(batch_id),
                position INTEGER NOT NULL CHECK (position >= 0),
                binding TEXT NOT NULL CHECK (length(binding) > 0),
                binding_revision TEXT NOT NULL CHECK (length(binding_revision) > 0),
                recovery TEXT NOT NULL CHECK (
                    recovery IN ('safe_to_replay', 'never_replay')
                ),
                request_json TEXT NOT NULL,
                status TEXT NOT NULL CHECK (
                    status IN (
                        'intent_committed', 'dispatch_started',
                        'settled', 'outcome_unknown'
                    )
                ),
                dispatch_count INTEGER NOT NULL CHECK (dispatch_count >= 0),
                outcome_json TEXT,
                UNIQUE (batch_id, position),
                CHECK ((status = 'settled') = (outcome_json IS NOT NULL))
             ) STRICT;
             INSERT INTO effects SELECT effect_id, batch_id, position, binding,
                binding_revision, recovery,
                coalesce(request_json, json_object('context', printf('%.*c', 262144, 'x'))),
                status, dispatch_count, outcome_json
             FROM effects_v4;
             DROP TABLE effects_v4;
             PRAGMA user_version = 3;
             PRAGMA wal_checkpoint(TRUNCATE);",
        )
        .expect("represent schema 3");
    let pages_before: i64 = connection
        .query_row("PRAGMA page_count", [], |row| row.get(0))
        .expect("page count");
    assert!(stored_request_bytes(&database) > 262_144);
    drop(connection);

    let kernel = Kernel::open(&database).expect("upgrade schema 3");
    let snapshot = kernel
        .inspect(session_id)
        .expect("inspect upgraded session");
    assert_eq!(
        snapshot.operations[0].effect_batches[0].effects[0].request, None,
        "a request of an operation finished before the upgrade is released"
    );
    assert_eq!(
        snapshot.operations[1].effect_batches[0].effects[0].request,
        Some(serde_json::json!({"work": "pending"})),
        "an unknown outcome keeps its request"
    );
    drop(kernel);
    let connection = open_connection(&database).expect("reopen upgraded database");
    let (version, pages, free): (i64, i64, i64) = connection
        .query_row(
            "SELECT (SELECT user_version FROM pragma_user_version),
                    (SELECT page_count FROM pragma_page_count),
                    (SELECT freelist_count FROM pragma_freelist_count)",
            [],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .expect("read upgraded storage");
    assert_eq!(version, 4);
    assert_eq!(free, 0, "the released space is returned");
    assert!(pages < pages_before, "{pages} pages, {pages_before} before");
}

/// Whether each effect still holds its request, in batch order.
fn requests_held(database: &std::path::Path) -> Vec<(String, bool)> {
    open_connection(database)
        .expect("open kernel database")
        .prepare(
            "SELECT e.binding, e.request_json IS NOT NULL FROM effects AS e
             JOIN effect_batches AS b ON b.batch_id = e.batch_id
             ORDER BY b.position, e.position",
        )
        .expect("prepare")
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
        .expect("query")
        .collect::<Result<_, _>>()
        .expect("effects")
}

/// Calls `first`, then `second`, then fails; it abandons and cancels on
/// request.
struct TwoBatchLoop(EffectRecovery);

impl LoopPlugin for TwoBatchLoop {
    fn decide(&self, input: LoopInput) -> Result<LoopDecision, LoopError> {
        let step = input.checkpoint.as_ref().map_or(0, |checkpoint| {
            checkpoint.state()["step"].as_u64().unwrap_or_default()
        });
        let checkpoint = Checkpoint::new(1, serde_json::json!({ "step": step + 1 }));
        let binding = match step {
            0 => "first",
            1 => "second",
            _ => {
                return Ok(LoopDecision::Fail {
                    checkpoint,
                    events: Vec::new(),
                    reason: "done".to_owned(),
                });
            }
        };
        Ok(LoopDecision::InvokeEffects {
            checkpoint,
            effects: vec![EffectRequest {
                binding: binding.to_owned(),
                request: serde_json::json!({ "call": binding }),
                recovery: if step == 0 {
                    EffectRecovery::SafeToReplay
                } else {
                    self.0
                },
            }],
        })
    }

    fn abandon_unknown_effect(
        &self,
        _input: UnknownEffectInput,
    ) -> Result<UnknownEffectAbandonment, LoopError> {
        Ok(UnknownEffectAbandonment {
            checkpoint: Checkpoint::new(1, serde_json::json!({ "abandoned": true })),
            events: Vec::new(),
        })
    }

    fn cancel_operation(
        &self,
        _input: CancellationInput,
    ) -> Result<CancellationTransition, LoopError> {
        Ok(CancellationTransition {
            checkpoint: Checkpoint::new(1, serde_json::json!({ "cancelled": true })),
            events: Vec::new(),
        })
    }
}

fn two_batch_runtime(recovery: EffectRecovery, second: Arc<dyn EffectAdapter>) -> Runtime {
    Runtime::new(
        LoopBinding::new("two-batch-loop", "1", Arc::new(TwoBatchLoop(recovery))),
        1,
        "two-batch-config-1",
        vec![
            EffectBinding::new(
                "first",
                "1",
                Arc::new(RecordingAdapter(Arc::new(Mutex::new(Vec::new())))),
            ),
            EffectBinding::new("second", "1", second),
        ],
    )
    .expect("valid runtime")
}

/// Succeeds after recording a cancellation of the operation it runs in.
struct CancellingAdapter {
    kernel: Weak<Kernel>,
    session_id: SessionId,
}

impl EffectAdapter for CancellingAdapter {
    fn invoke(&self, _invocation: EffectInvocation) -> EffectFuture<'_> {
        let kernel = self.kernel.upgrade().expect("kernel still live");
        let operation_id =
            kernel.inspect(self.session_id).expect("inspect").operations[0].operation_id;
        kernel
            .request_cancellation(self.session_id, operation_id, CancellationId::new())
            .expect("request cancellation");
        Box::pin(std::future::ready(
            EffectOutcome::Success(serde_json::json!({ "ok": true })).into(),
        ))
    }
}

#[tokio::test]
async fn a_failed_operation_releases_every_batch() {
    let directory = tempdir().expect("temporary directory");
    let database = directory.path().join("kernel.sqlite3");
    let (kernel, session_id) = kernel_with_command(&database);
    let runtime = two_batch_runtime(
        EffectRecovery::SafeToReplay,
        Arc::new(RecordingAdapter(Arc::new(Mutex::new(Vec::new())))),
    );
    assert!(matches!(
        kernel.drive(session_id, &runtime).await.expect("drive"),
        DriveResult::Finished {
            outcome: OperationOutcome::Failed { .. },
            ..
        }
    ));
    assert_eq!(
        requests_held(&database),
        [("first".to_owned(), false), ("second".to_owned(), false)]
    );
}

#[tokio::test]
async fn a_cancelled_operation_releases_its_settled_batches() {
    let directory = tempdir().expect("temporary directory");
    let database = directory.path().join("kernel.sqlite3");
    let (kernel, session_id) = kernel_with_command(&database);
    let kernel = Arc::new(kernel);
    let runtime = two_batch_runtime(
        EffectRecovery::SafeToReplay,
        Arc::new(CancellingAdapter {
            kernel: Arc::downgrade(&kernel),
            session_id,
        }),
    );
    kernel.drive(session_id, &runtime).await.expect("drive");
    assert_eq!(
        kernel.inspect(session_id).expect("inspect").operations[0].status,
        OperationStatus::Cancelled
    );
    assert_eq!(
        requests_held(&database),
        [("first".to_owned(), false), ("second".to_owned(), false)]
    );
}

#[tokio::test]
async fn abandonment_releases_earlier_batches_and_keeps_the_unknown_one() {
    let directory = tempdir().expect("temporary directory");
    let database = directory.path().join("kernel.sqlite3");
    let (kernel, session_id) = kernel_with_command(&database);
    let runtime = two_batch_runtime(EffectRecovery::NeverReplay, Arc::new(UnknownAdapter));
    let DriveResult::Blocked { operation_id } =
        kernel.drive(session_id, &runtime).await.expect("drive")
    else {
        panic!("an unknown outcome blocks the operation")
    };
    assert_eq!(
        requests_held(&database),
        [("first".to_owned(), true), ("second".to_owned(), true)],
        "a blocked operation keeps every request"
    );
    let unsettled_release = open_connection(&database)
        .expect("open kernel database")
        .execute(
            "UPDATE effects SET request_json = NULL WHERE binding = 'second'",
            [],
        );
    assert!(
        unsettled_release.is_err(),
        "only a settled effect may lose its request"
    );

    kernel
        .abandon_unknown_effect(session_id, operation_id, &runtime)
        .expect("abandon");
    assert_eq!(
        requests_held(&database),
        [("first".to_owned(), false), ("second".to_owned(), true)]
    );
}

#[test]
fn a_kernel_without_the_release_fails_to_open() {
    let directory = tempdir().expect("temporary directory");
    let database = directory.path().join("kernel.sqlite3");
    drop(Kernel::open(&database).expect("create kernel"));
    open_connection(&database)
        .expect("open kernel database")
        .execute_batch("DROP TRIGGER release_finished_effect_requests;")
        .expect("drop the release");
    assert!(matches!(
        Kernel::open(&database),
        Err(KernelError::Corrupt(_))
    ));
}
