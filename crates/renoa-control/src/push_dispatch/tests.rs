use std::{
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
    time::Duration,
};

use tokio::time::{Instant, timeout};
use tokio_util::sync::CancellationToken;

use super::supervise;

const WAIT: Duration = Duration::from_secs(10);

async fn eventually(what: &str, done: impl Fn() -> bool) {
    let deadline = Instant::now() + WAIT;
    while !done() {
        assert!(Instant::now() < deadline, "{what}");
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}

#[tokio::test]
async fn a_dispatcher_that_dies_is_started_again_until_shutdown() {
    let starts = Arc::new(AtomicUsize::new(0));
    let stop = CancellationToken::new();
    let supervisor = tokio::spawn(supervise(
        {
            let starts = Arc::clone(&starts);
            let stop = stop.clone();
            move || {
                let first = starts.fetch_add(1, Ordering::SeqCst) == 0;
                let stop = stop.clone();
                tokio::spawn(async move {
                    assert!(!first, "the first dispatcher dies");
                    stop.cancelled().await;
                })
            }
        },
        stop.clone(),
    ));
    eventually("a second dispatcher starts", || {
        starts.load(Ordering::SeqCst) == 2
    })
    .await;
    stop.cancel();
    timeout(WAIT, supervisor)
        .await
        .expect("the supervisor returns at shutdown")
        .expect("the supervisor does not panic");
    assert_eq!(
        starts.load(Ordering::SeqCst),
        2,
        "none starts after shutdown"
    );
}

/// Set when the task holding it is dropped, as an aborted task is.
struct Dropped(Arc<AtomicBool>);

impl Drop for Dropped {
    fn drop(&mut self) {
        self.0.store(true, Ordering::SeqCst);
    }
}

#[tokio::test]
async fn dropping_the_supervisor_aborts_its_dispatcher() {
    let started = Arc::new(AtomicBool::new(false));
    let dropped = Arc::new(AtomicBool::new(false));
    let supervisor = tokio::spawn(supervise(
        {
            let started = Arc::clone(&started);
            let dropped = Arc::clone(&dropped);
            move || {
                let started = Arc::clone(&started);
                let held = Dropped(Arc::clone(&dropped));
                tokio::spawn(async move {
                    let _held = held;
                    started.store(true, Ordering::SeqCst);
                    // A dispatcher that never sees a shutdown.
                    std::future::pending::<()>().await;
                })
            }
        },
        CancellationToken::new(),
    ));
    eventually("the dispatcher starts", || started.load(Ordering::SeqCst)).await;
    supervisor.abort();
    let _ = supervisor.await;
    eventually("the dispatcher is aborted with its supervisor", || {
        dropped.load(Ordering::SeqCst)
    })
    .await;
}
