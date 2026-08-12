use std::{
    sync::{Arc, Condvar, Mutex},
    time::Duration,
};

use tempfile::tempdir;
use tokio::sync::oneshot;

use crate::{Harness, HarnessError, SessionId, store::blocking_transition};

#[tokio::test]
async fn aborting_a_blocking_transition_does_not_release_session_ownership_early() {
    let directory = tempdir().expect("temporary directory");
    let database = directory.path().join("harness.sqlite3");
    let harness = Harness::open(&database).expect("open harness");
    let session_id = SessionId::new();
    let lease = harness.begin_run(session_id).expect("own session");
    let (entered_tx, entered_rx) = oneshot::channel();
    let release = Arc::new((Mutex::new(false), Condvar::new()));
    let worker_release = Arc::clone(&release);

    let task = tokio::spawn(blocking_transition(lease, move || {
        entered_tx.send(()).expect("signal worker entry");
        let (released, condition) = &*worker_release;
        let mut released = released.lock().expect("release lock");
        while !*released {
            released = condition.wait(released).expect("wait for release");
        }
        Ok(())
    }));
    entered_rx.await.expect("blocking transition entered");
    task.abort();
    assert!(task.await.expect_err("aborted waiter").is_cancelled());

    assert_eq!(
        harness
            .begin_run(session_id)
            .err()
            .expect("worker must retain ownership"),
        HarnessError::Busy(session_id)
    );

    let (released, condition) = &*release;
    *released.lock().expect("release lock") = true;
    condition.notify_one();
    let replacement = tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            match harness.begin_run(session_id) {
                Ok(lease) => break lease,
                Err(HarnessError::Busy(_)) => tokio::task::yield_now().await,
                Err(error) => panic!("unexpected ownership error: {error}"),
            }
        }
    })
    .await
    .expect("blocking worker released ownership");
    drop(replacement);
}
