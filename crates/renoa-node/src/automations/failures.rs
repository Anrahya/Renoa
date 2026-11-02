//! The Host failures the schedule outlives, and the run of them that ends it.

use renoa_local::LocalHostError;

use super::host_error;
use crate::{bridge::NodeError, node_log};

/// Consecutive Host failures, a busy catalog aside, that end the scheduler.
pub(super) const FAILURES: u32 = 5;

/// The Host failures, a busy catalog aside, the schedule met in a row.
#[derive(Default)]
pub(super) struct Failures(u32);

impl Failures {
    /// Logs `error`, which the next tick asks again after. A busy catalog
    /// always is; any other failure counts, and the [`FAILURES`]th in a row
    /// is returned, to end the scheduler.
    pub(super) fn met(&mut self, error: &LocalHostError) -> Result<(), NodeError> {
        if error.catalog_busy() {
            busy(error);
            return Ok(());
        }
        self.0 = self.0.saturating_add(1);
        node_log::event(
            "error",
            "automation_schedule_unavailable",
            &serde_json::json!({
                "error": error.to_string(),
                "busy": false,
                "failures": self.0,
            }),
        );
        if self.0 >= FAILURES {
            return Err(host_error(error));
        }
        Ok(())
    }
}

/// Logs a busy catalog, which is asked again after a pause.
pub(super) fn busy(error: &LocalHostError) {
    node_log::event(
        "warn",
        "automation_schedule_unavailable",
        &serde_json::json!({ "error": error.to_string(), "busy": true }),
    );
}

#[cfg(test)]
mod tests {
    use renoa_local::{HostCatalogError, LocalHostError};

    use super::{FAILURES, Failures};

    /// The failure of a write while another connection holds the catalog,
    /// or, unless `held`, of a read of a missing table.
    fn failure(held: bool) -> LocalHostError {
        let directory = tempfile::tempdir().expect("directory");
        let path = directory.path().join("catalog.sqlite3");
        let holder = rusqlite::Connection::open(&path).expect("holder");
        let other = rusqlite::Connection::open(&path).expect("other");
        other
            .busy_timeout(std::time::Duration::ZERO)
            .expect("no busy wait");
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
    fn a_busy_catalog_never_ends_the_scheduler_and_any_other_failure_does_in_a_row() {
        let mut failures = Failures::default();
        for _ in 0..FAILURES * 2 {
            failures
                .met(&failure(true))
                .expect("a busy catalog is asked again");
        }
        for _ in 1..FAILURES {
            failures
                .met(&failure(false))
                .expect("asked again until the limit");
        }
        let ended = failures.met(&failure(false));
        assert!(
            ended.is_err_and(|error| error.to_string().contains("missing")),
            "the limit ends the scheduler"
        );
    }
}
