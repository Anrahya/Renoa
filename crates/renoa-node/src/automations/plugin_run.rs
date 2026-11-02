//! A compiled plugin's run, in flight beside the schedule: one at a time,
//! recording how it ended itself, aborted when dropped and waited for when
//! the node drains.

use std::{sync::Arc, time::Duration};

use renoa_local::{AutomationScheduler, LocalHostError, ScheduledRun};
use tokio::{task::JoinHandle, time::Instant};
use tokio_util::sync::CancellationToken;

use super::now_ms;
use crate::{
    bridge::{NodeError, NodeRuntime},
    node_log,
};

/// A plugin's run in flight beside the schedule. Dropping it aborts the run,
/// which stays admitted for the next scheduler to resume.
pub(super) struct PluginTask(JoinHandle<Result<(), NodeError>>);

impl Drop for PluginTask {
    fn drop(&mut self) {
        self.0.abort();
    }
}

/// Starts `due`, a run of `plugin`'s trigger, beside the schedule; it records
/// how the run ended itself.
pub(super) fn start(
    runtime: &Arc<NodeRuntime>,
    scheduler: &Arc<AutomationScheduler>,
    due: ScheduledRun,
    plugin: &'static str,
) -> Result<PluginTask, NodeError> {
    let (runtime, scheduler) = (Arc::clone(runtime), Arc::clone(scheduler));
    let now = now_ms()?;
    Ok(PluginTask(tokio::spawn(Box::pin(async move {
        let finished = scheduler
            .run_for_plugin(&runtime.host, &due, now)
            .await
            .map_err(|error| host_error(&error))?;
        // A run the draining Host stopped part way stays admitted, and the
        // next scheduler resumes it.
        if runtime.host.memory_runs_stopped() {
            node_log::event(
                "info",
                "automation_left_for_restart",
                &serde_json::json!({
                    "automation_id": due.run.automation_id,
                    "run_id": due.run.id,
                    "plugin": plugin,
                }),
            );
            return Ok(());
        }
        let status = finished.status();
        scheduler
            .finish_plugin_run(finished, now_ms()?)
            .await
            .map_err(|error| host_error(&error))?;
        node_log::event(
            "info",
            "automation_finished",
            &serde_json::json!({
                "automation_id": due.run.automation_id,
                "run_id": due.run.id,
                "plugin": plugin,
                "status": status,
            }),
        );
        Ok(())
    }))))
}

/// Takes the plugin run in flight once it ended, with how it ended; `None`
/// while none ended.
pub(super) async fn reap(plugin: &mut Option<PluginTask>) -> Option<Result<(), NodeError>> {
    if !plugin.as_ref().is_some_and(|task| task.0.is_finished()) {
        return None;
    }
    let mut task = plugin.take()?;
    Some(ended(&mut task).await)
}

/// The longest a plugin run that keeps failing is held back.
const RETRY_CAP: Duration = Duration::from_secs(300);

/// How long plugin runs are held back after a plugin run failed, or
/// panicked, in a row: it stays admitted, and is handed out again after a
/// delay that doubles with each failure up to [`RETRY_CAP`].
#[derive(Default)]
pub(super) struct Retry {
    failures: u32,
    until: Option<Instant>,
}

impl Retry {
    /// Records how the plugin run that `reap` took ended.
    pub(super) fn ended(&mut self, ended: Result<(), NodeError>) {
        match ended {
            Ok(()) => *self = Self::default(),
            Err(error) => self.failed(&error),
        }
    }

    /// Holds plugin runs back after one more failure, and logs it.
    fn failed(&mut self, error: &NodeError) {
        self.failures = self.failures.saturating_add(1);
        let delay = super::PAUSE
            .saturating_mul(2_u32.saturating_pow(self.failures.min(16)))
            .min(RETRY_CAP);
        self.until = Some(Instant::now() + delay);
        node_log::event(
            "warn",
            "automation_plugin_run_failed",
            &serde_json::json!({
                "error": error.to_string(),
                "failures": self.failures,
                "retry_ms": u64::try_from(delay.as_millis()).unwrap_or(u64::MAX),
            }),
        );
    }

    /// Whether no plugin run is handed out now.
    pub(super) fn holds(&self) -> bool {
        self.until.is_some_and(|until| Instant::now() < until)
    }
}

/// For a draining node: stops the Host's memory runs, so the note pass in
/// flight settles as cancelled and a nightly run stops before its next job,
/// and waits for `task` to end, or for shutdown, which drops it.
pub(super) async fn drain(
    runtime: &NodeRuntime,
    mut task: PluginTask,
    shutdown: &CancellationToken,
) -> Result<(), NodeError> {
    runtime.host.stop_memory_runs();
    tokio::select! {
        () = shutdown.cancelled() => Ok(()),
        ended = ended(&mut task) => ended,
    }
}

fn host_error(error: &LocalHostError) -> NodeError {
    NodeError::Store(error.to_string())
}

async fn ended(task: &mut PluginTask) -> Result<(), NodeError> {
    (&mut task.0)
        .await
        .map_err(|error| NodeError::Task(error.to_string()))?
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::{PluginTask, RETRY_CAP, Retry, reap};
    use crate::bridge::NodeError;

    fn panics() -> Result<(), NodeError> {
        panic!("the plugin run panics")
    }

    /// The run in flight, once it ended.
    async fn reaped(plugin: &mut Option<PluginTask>) -> Result<(), NodeError> {
        loop {
            if let Some(ended) = reap(plugin).await {
                return ended;
            }
            tokio::task::yield_now().await;
        }
    }

    #[tokio::test(start_paused = true)]
    async fn a_plugin_run_that_panics_every_time_is_held_back_longer_each_time() {
        let mut retry = Retry::default();
        let mut delays = Vec::new();
        for _ in 0..10 {
            let mut plugin = Some(PluginTask(tokio::spawn(async { panics() })));
            let ended = reaped(&mut plugin).await;
            assert!(plugin.is_none(), "taken once it ended");
            let started = tokio::time::Instant::now();
            retry.ended(ended);
            assert!(retry.holds(), "held back after a failure");
            let until = retry.until.expect("held until");
            delays.push(until - started);
            tokio::time::advance(until - started).await;
            assert!(!retry.holds(), "handed out again after the delay");
        }
        let seconds = Duration::from_secs;
        assert_eq!(
            delays[..5],
            [seconds(2), seconds(4), seconds(8), seconds(16), seconds(32)]
        );
        assert_eq!(delays[9], RETRY_CAP);
        assert_eq!(retry.failures, 10, "every failure is counted");
        retry.ended(Ok(()));
        assert!(!retry.holds());
        assert_eq!(retry.failures, 0, "a run that ends starts the count again");
    }
}
