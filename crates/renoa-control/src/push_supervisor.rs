//! Keeping the push dispatcher running for exactly as long as `serve` does.
//!
//! A dispatcher that ends before shutdown, by a panic or otherwise, is started
//! again after a delay that doubles with each consecutive failure, from one
//! second up to a minute, and falls back to a second after a dispatcher has
//! run healthy for five minutes. Every dispatcher shares one [`Pacing`], so
//! a restart does not cost a device its gap or its backoff. The running dispatcher is
//! aborted when the supervising future is dropped, so none outlives `serve`.

use std::time::Duration;

use tokio::{task::JoinHandle, time::Instant};
use tokio_util::sync::CancellationToken;

use crate::{
    control_log,
    push_pacing::{Pacing, doubled},
};

const FIRST_RESTART_DELAY: Duration = Duration::from_secs(1);
const MAX_RESTART_DELAY: Duration = Duration::from_mins(1);
/// A dispatcher that ran this long before it died starts the count afresh.
const HEALTHY_RUN: Duration = Duration::from_mins(5);

/// Runs a dispatcher from `start` until `shutdown`, and starts another
/// whenever one ends before it. `start` receives the shared [`Pacing`].
pub(crate) async fn supervise(
    mut start: impl FnMut(Pacing) -> JoinHandle<()>,
    shutdown: CancellationToken,
) {
    let pacing = Pacing::default();
    let mut failures: u32 = 0;
    loop {
        let started = Instant::now();
        let mut running = AbortOnDrop(start(pacing.clone()));
        let ended = tokio::select! {
            ended = &mut running.0 => ended,
            () = shutdown.cancelled() => {
                // The dispatcher watches the same token and returns.
                let _ = (&mut running.0).await;
                return;
            }
        };
        if shutdown.is_cancelled() {
            return;
        }
        if started.elapsed() >= HEALTHY_RUN {
            failures = 0;
        }
        failures = failures.saturating_add(1);
        let delay = restart_delay(failures);
        let reason = match ended {
            Ok(()) => "the dispatcher returned before shutdown",
            Err(error) if error.is_panic() => "the dispatcher panicked",
            Err(_) => "the dispatcher was cancelled",
        };
        control_log::event(
            "error",
            "push_dispatcher_failed",
            &serde_json::json!({
                "reason": reason,
                "consecutive_failures": failures,
                "restart_after_ms": delay.as_millis(),
            }),
        );
        tokio::select! {
            () = shutdown.cancelled() => return,
            () = tokio::time::sleep(delay) => {}
        }
    }
}

/// One second after the first failure in a row, doubling up to a minute.
fn restart_delay(failures: u32) -> Duration {
    doubled(FIRST_RESTART_DELAY, failures, MAX_RESTART_DELAY)
}

struct AbortOnDrop(JoinHandle<()>);

impl Drop for AbortOnDrop {
    fn drop(&mut self) {
        self.0.abort();
    }
}

#[cfg(test)]
mod tests;
