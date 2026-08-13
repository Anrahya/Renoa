use std::{num::NonZeroU32, sync::Arc, time::Duration};

use renoa_agent::ContentBlock;
use renoa_harness::{
    CancellationId, Harness, HarnessError, OperationRequest, RequestId, RuntimeProfile, SessionId,
};
use tempfile::tempdir;

use super::support::PendingModel;

#[tokio::test]
async fn retrying_an_old_cancellation_cannot_stop_the_next_operation() {
    let directory = tempdir().expect("temporary directory");
    let database = directory.path().join("harness.sqlite3");
    let harness = Arc::new(Harness::open(&database).expect("open harness"));
    let session_id = SessionId::new();
    harness
        .create_standalone_session(session_id)
        .await
        .expect("create session");
    let first = harness
        .admit_standalone(
            session_id,
            OperationRequest::new(RequestId::new(), vec![ContentBlock::text("first")]),
        )
        .await
        .expect("admit first operation");
    let first_model = Arc::new(PendingModel::default());
    let first_profile = RuntimeProfile::new(
        "coding-v1",
        first_model.clone(),
        "Be precise.",
        NonZeroU32::new(2).expect("non-zero attempt limit"),
    );
    let driver = Arc::clone(&harness);
    let first_run = tokio::spawn(async move { driver.run_next(session_id, &first_profile).await });
    first_model.started.notified().await;
    let first_cancellation = CancellationId::new();
    harness
        .request_standalone_cancellation(session_id, first.operation_id, first_cancellation)
        .await
        .expect("cancel first operation");
    first_run
        .await
        .expect("join first driver")
        .expect("settle first cancellation");

    let second = harness
        .admit_standalone(
            session_id,
            OperationRequest::new(RequestId::new(), vec![ContentBlock::text("second")]),
        )
        .await
        .expect("admit second operation");
    let second_model = Arc::new(PendingModel::default());
    let second_profile = RuntimeProfile::new(
        "coding-v1",
        second_model.clone(),
        "Be precise.",
        NonZeroU32::new(2).expect("non-zero attempt limit"),
    );
    let driver = Arc::clone(&harness);
    let mut second_run =
        tokio::spawn(async move { driver.run_next(session_id, &second_profile).await });
    second_model.started.notified().await;

    harness
        .request_standalone_cancellation(session_id, first.operation_id, first_cancellation)
        .await
        .expect("retry first cancellation");
    assert!(
        tokio::time::timeout(Duration::from_millis(50), &mut second_run)
            .await
            .is_err(),
        "retrying old cancellation must not signal current work"
    );

    harness
        .request_standalone_cancellation(session_id, second.operation_id, CancellationId::new())
        .await
        .expect("cancel second operation");
    second_run
        .await
        .expect("join second driver")
        .expect("settle second cancellation");
}

#[tokio::test]
async fn cancellation_identity_and_active_target_are_enforced() {
    let directory = tempdir().expect("temporary directory");
    let database = directory.path().join("harness.sqlite3");
    let harness = Arc::new(Harness::open(&database).expect("open harness"));
    let session_id = SessionId::new();
    harness
        .create_standalone_session(session_id)
        .await
        .expect("create session");
    let first = harness
        .admit_standalone(
            session_id,
            OperationRequest::new(RequestId::new(), vec![ContentBlock::text("first")]),
        )
        .await
        .expect("admit first operation");
    let second = harness
        .admit_standalone(
            session_id,
            OperationRequest::new(RequestId::new(), vec![ContentBlock::text("second")]),
        )
        .await
        .expect("admit second operation");
    let model = Arc::new(PendingModel::default());
    let profile = RuntimeProfile::new(
        "coding-v1",
        model.clone(),
        "Be precise.",
        NonZeroU32::new(2).expect("non-zero attempt limit"),
    );
    let driver = Arc::clone(&harness);
    let run = tokio::spawn(async move { driver.run_next(session_id, &profile).await });
    model.started.notified().await;

    assert_eq!(
        harness
            .request_standalone_cancellation(
                session_id,
                second.operation_id,
                CancellationId::new(),
            )
            .await
            .expect_err("queued operation must not be cancellable"),
        HarnessError::OperationNotCancellable(second.operation_id)
    );
    let cancellation_id = CancellationId::new();
    harness
        .request_standalone_cancellation(session_id, first.operation_id, cancellation_id)
        .await
        .expect("cancel active operation");
    run.await
        .expect("join first driver")
        .expect("settle first cancellation");
    assert_eq!(
        harness
            .request_standalone_cancellation(session_id, first.operation_id, CancellationId::new(),)
            .await
            .expect_err("terminal operation must reject a new cancellation"),
        HarnessError::OperationNotCancellable(first.operation_id)
    );

    let second_model = Arc::new(PendingModel::default());
    let second_profile = RuntimeProfile::new(
        "coding-v1",
        second_model.clone(),
        "Be precise.",
        NonZeroU32::new(2).expect("non-zero attempt limit"),
    );
    let driver = Arc::clone(&harness);
    let second_run =
        tokio::spawn(async move { driver.run_next(session_id, &second_profile).await });
    second_model.started.notified().await;
    assert_eq!(
        harness
            .request_standalone_cancellation(session_id, second.operation_id, cancellation_id,)
            .await
            .expect_err("cancellation identity must stay bound to its first target"),
        HarnessError::CancellationConflict {
            cancellation_id,
            operation_id: first.operation_id,
        }
    );
    harness
        .request_standalone_cancellation(session_id, second.operation_id, CancellationId::new())
        .await
        .expect("cancel second operation");
    second_run
        .await
        .expect("join second driver")
        .expect("settle second cancellation");
}
