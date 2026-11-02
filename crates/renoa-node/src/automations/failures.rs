//! The failures the schedule outlives, and the heartbeat that shows when it
//! keeps failing.
//!
//! A schedule that keeps failing writes no heartbeat, so Host inspection
//! shows `automation_scheduler.heartbeat_ms` going stale, and asks again
//! less often, up to [`RETRY_CAP`] apart. The node and its turns stay up.

use std::{
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};

use renoa_local::{AutomationScheduler, LocalHostError};
use tokio_util::sync::CancellationToken;

use super::{HEARTBEAT, PAUSE, now_ms};
use crate::{bridge::NodeError, node_log};

/// Host failures in a row, a busy catalog aside, from which the schedule is
/// failing.
pub(super) const FAILURES: u32 = 5;

/// The longest a failing schedule waits before asking again.
const RETRY_CAP: Duration = Duration::from_secs(60);

/// The failures, a busy catalog aside, the schedule met in a row, and
/// whether it is failing, which the heartbeat reads.
pub(super) struct Failures {
    count: u32,
    failing: Arc<AtomicBool>,
}

impl Failures {
    pub(super) fn new(failing: Arc<AtomicBool>) -> Self {
        Self { count: 0, failing }
    }

    /// Logs `error` and returns how long to wait before asking again. A busy
    /// catalog is asked again after [`PAUSE`] for as long as it stays busy;
    /// any other failure counts.
    pub(super) fn met(&mut self, error: &LocalHostError) -> Duration {
        if error.catalog_busy() {
            node_log::event(
                "warn",
                "automation_schedule_unavailable",
                &serde_json::json!({ "error": error.to_string(), "busy": true }),
            );
            return PAUSE;
        }
        self.failed(&error.to_string())
    }

    /// Logs a failure that stopped the schedule, which is failing at once,
    /// and returns how long to wait before starting it again.
    pub(super) fn stopped(&mut self, error: &NodeError) -> Duration {
        self.count = self.count.max(FAILURES - 1);
        self.failed(&error.to_string())
    }

    /// Counts a failure. From the [`FAILURES`]th in a row the schedule is
    /// failing, and each further one doubles the wait, up to [`RETRY_CAP`].
    fn failed(&mut self, error: &str) -> Duration {
        self.count = self.count.saturating_add(1);
        let Some(over) = self.count.checked_sub(FAILURES) else {
            node_log::event(
                "error",
                "automation_schedule_unavailable",
                &serde_json::json!({ "error": error, "busy": false, "failures": self.count }),
            );
            return PAUSE;
        };
        self.failing.store(true, Ordering::Relaxed);
        let delay = PAUSE
            .saturating_mul(2_u32.saturating_pow(over.saturating_add(1)))
            .min(RETRY_CAP);
        node_log::event(
            "error",
            "automation_schedule_failing",
            &serde_json::json!({
                "error": error,
                "failures": self.count,
                "retry_ms": u64::try_from(delay.as_millis()).unwrap_or(u64::MAX),
            }),
        );
        delay
    }

    /// A tick that went well. Returns whether the schedule was failing, so
    /// its heartbeat is written again.
    pub(super) fn cleared(&mut self) -> bool {
        self.count = 0;
        let was_failing = self.failing.swap(false, Ordering::Relaxed);
        if was_failing {
            node_log::event(
                "info",
                "automation_schedule_recovered",
                &serde_json::json!({}),
            );
        }
        was_failing
    }
}

/// Writes the heartbeat every [`HEARTBEAT`] while the schedule is not
/// failing, until shutdown.
pub(super) async fn heartbeat(
    scheduler: &AutomationScheduler,
    shutdown: &CancellationToken,
    failing: &AtomicBool,
) {
    let mut ticks = tokio::time::interval(HEARTBEAT);
    loop {
        tokio::select! {
            () = shutdown.cancelled() => return,
            _ = ticks.tick() => {}
        }
        if !failing.load(Ordering::Relaxed) {
            beat(scheduler).await;
        }
    }
}

/// Records that the scheduler is alive; a failure to is logged.
pub(super) async fn beat(scheduler: &AutomationScheduler) {
    let written = match now_ms() {
        Ok(now) => scheduler
            .heartbeat(now)
            .await
            .map_err(|error| error.to_string()),
        Err(error) => Err(error.to_string()),
    };
    if let Err(error) = written {
        node_log::event(
            "warn",
            "automation_heartbeat_failed",
            &serde_json::json!({ "error": error }),
        );
    }
}

#[cfg(test)]
mod tests {
    use std::{
        sync::{
            Arc,
            atomic::{AtomicBool, Ordering},
        },
        time::Duration,
    };

    use renoa_local::{HostCatalogError, LocalHostError};

    use super::{FAILURES, Failures, PAUSE};
    use crate::bridge::NodeError;

    /// The failure of a write while another connection holds the catalog,
    /// or, unless `held`, of a read of a missing table.
    fn failure(held: bool) -> LocalHostError {
        let directory = tempfile::tempdir().expect("directory");
        let path = directory.path().join("catalog.sqlite3");
        let holder = rusqlite::Connection::open(&path).expect("holder");
        let other = rusqlite::Connection::open(&path).expect("other");
        other.busy_timeout(Duration::ZERO).expect("no busy wait");
        let error = if held {
            holder
                .execute_batch("CREATE TABLE t(x); BEGIN EXCLUSIVE")
                .expect("hold the catalog");
            other.execute_batch("INSERT INTO t VALUES (1)")
        } else {
            other.execute_batch("SELECT * FROM missing")
        };
        LocalHostError::HostCatalog(HostCatalogError::Database(error.expect_err("refused")))
    }

    #[test]
    fn only_failures_in_a_row_make_the_schedule_failing_and_back_it_off_to_a_minute() {
        let failing = Arc::new(AtomicBool::new(false));
        let mut failures = Failures::new(Arc::clone(&failing));
        for _ in 0..FAILURES * 2 {
            assert_eq!(failures.met(&failure(true)), PAUSE, "busy asks again soon");
        }
        assert!(
            !failing.load(Ordering::Relaxed),
            "a busy catalog is not failing"
        );
        for _ in 1..FAILURES {
            assert_eq!(failures.met(&failure(false)), PAUSE);
        }
        assert!(!failing.load(Ordering::Relaxed), "not yet failing");
        let delays: Vec<u64> = (0..7)
            .map(|_| failures.met(&failure(false)).as_secs())
            .collect();
        assert_eq!(delays, [2, 4, 8, 16, 32, 60, 60]);
        assert!(failing.load(Ordering::Relaxed), "failing from the limit");

        assert!(failures.cleared(), "a good tick ends the failing");
        assert!(!failing.load(Ordering::Relaxed));
        assert!(!failures.cleared(), "only once");
        assert_eq!(
            failures.met(&failure(false)),
            PAUSE,
            "the count starts again"
        );
        assert!(!failing.load(Ordering::Relaxed));
    }

    #[test]
    fn a_stopped_schedule_is_failing_at_once() {
        let failing = Arc::new(AtomicBool::new(false));
        let mut failures = Failures::new(Arc::clone(&failing));
        let stopped = NodeError::Protocol("refused".to_owned());
        assert_eq!(failures.stopped(&stopped).as_secs(), 2);
        assert!(failing.load(Ordering::Relaxed));
        assert_eq!(failures.stopped(&stopped).as_secs(), 4);
    }
}
