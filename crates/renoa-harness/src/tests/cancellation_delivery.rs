use std::{
    num::NonZeroU32,
    sync::{
        Arc, Condvar, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};

use renoa_agent::ContentBlock;
use tempfile::tempdir;
use tokio::sync::oneshot;

use super::support::{NeverCalledModel, create_session};
use crate::{CancellationId, Harness, OperationRequest, RequestId, RuntimeProfile};

#[tokio::test]
async fn aborting_the_cancellation_waiter_cannot_drop_its_post_commit_signal() {
    let directory = tempdir().expect("temporary directory");
    let database = directory.path().join("harness.sqlite3");
    let harness = Arc::new(Harness::open(&database).expect("open harness"));
    let session_id = create_session(&harness).await;
    let admission = harness
        .admit_standalone(
            session_id,
            OperationRequest::new(RequestId::new(), vec![ContentBlock::text("keep working")]),
        )
        .await
        .expect("admit operation");
    let lease = harness.begin_run(session_id).expect("own session");
    let profile = RuntimeProfile::new(
        "coding-v1",
        Arc::new(NeverCalledModel),
        "Be precise.",
        NonZeroU32::new(2).expect("non-zero attempt limit"),
    );
    harness
        .store
        .activate(&lease, session_id, profile.frozen())
        .await
        .expect("activate")
        .expect("active operation");

    let cancellation_id = CancellationId::new();
    let (entered_tx, entered_rx) = oneshot::channel();
    let release = Arc::new((Mutex::new(false), Condvar::new()));
    let worker_release = Arc::clone(&release);
    let signalled = Arc::new(AtomicBool::new(false));
    let worker_signalled = Arc::clone(&signalled);
    let worker = Arc::clone(&harness);
    let request = tokio::spawn(async move {
        worker
            .store
            .request_cancellation(
                session_id,
                admission.operation_id,
                cancellation_id,
                move || {
                    entered_tx.send(()).expect("signal post-commit entry");
                    let (released, condition) = &*worker_release;
                    let mut released = released.lock().expect("release lock");
                    while !*released {
                        released = condition.wait(released).expect("wait for release");
                    }
                    worker_signalled.store(true, Ordering::SeqCst);
                    Ok(())
                },
            )
            .await
    });
    entered_rx.await.expect("cancellation committed");
    harness
        .request_standalone_cancellation(session_id, admission.operation_id, cancellation_id)
        .await
        .expect("committed cancellation must already be retryable");
    request.abort();
    assert!(request.await.expect_err("aborted waiter").is_cancelled());

    let (released, condition) = &*release;
    *released.lock().expect("release lock") = true;
    condition.notify_one();
    tokio::time::timeout(Duration::from_secs(2), async {
        while !signalled.load(Ordering::SeqCst) {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("post-commit signal survived waiter cancellation");
}
