//! A compiled plugin's run, in flight beside the schedule: one at a time,
//! recording how it ended itself, aborted when dropped and waited for when
//! the node drains.

use std::sync::Arc;

use renoa_local::{AutomationScheduler, ScheduledRun};
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;

use super::{host_error, now_ms};
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

/// Takes the plugin run in flight once it ended, with its error.
pub(super) async fn reap(plugin: &mut Option<PluginTask>) -> Result<(), NodeError> {
    if !plugin.as_ref().is_some_and(|task| task.0.is_finished()) {
        return Ok(());
    }
    let Some(mut task) = plugin.take() else {
        return Ok(());
    };
    ended(&mut task).await
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

async fn ended(task: &mut PluginTask) -> Result<(), NodeError> {
    (&mut task.0)
        .await
        .map_err(|error| NodeError::Task(error.to_string()))?
}
