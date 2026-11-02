//! The Host side of automation scheduling: admitting due runs and recording
//! their results. The Host does not execute an agent's runs; its scheduler
//! hands each one to an execution node and records the result the node
//! reports. A run of a plugin's trigger is the plugin's, and the Host runs it.

use std::{path::PathBuf, sync::Arc};

use uuid::Uuid;

use super::{
    AutomationRun, LocalHost, LocalHostError, RunOutcome, RunStatus, retention, runs, store,
};
use crate::{
    host::{catalog, lease::ExecutionLease},
    plugins::host::{
        HostPluginId,
        triggers::{self, TriggerContext},
    },
};

/// One admitted run and where it goes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ScheduledRun {
    pub run: AutomationRun,
    /// The Host session the run goes to: the one the automation was created
    /// in, when its own agent created it from one, or a reminder's
    /// conversation with its person. `None` runs in a conversation of the
    /// automation's own.
    pub origin_session_id: Option<Uuid>,
    /// The compiled plugin that runs this run of its trigger. Its run is not
    /// sent to the agent: [`AutomationScheduler::run_for_plugin`] runs it on
    /// the Host. `None` for a run sent to the agent, a reminder's included.
    pub plugin: Option<&'static str>,
}

/// How a run of a plugin's trigger ended, to be recorded with
/// [`AutomationScheduler::finish_plugin_run`].
#[derive(Debug)]
#[must_use]
pub struct PluginRun {
    run: Uuid,
    ended: triggers::Ended,
}

impl PluginRun {
    #[must_use]
    pub const fn status(&self) -> RunStatus {
        self.ended.status
    }
}

/// Exclusive ownership of a Host's automation schedule. Dropping it releases
/// the schedule to the next owner.
pub struct AutomationScheduler {
    database: PathBuf,
    triggers: Arc<TriggerContext>,
    _lease: ExecutionLease,
}

impl LocalHost {
    /// Takes ownership of this Host's automation schedule. One process owns it
    /// at a time, so a run is admitted once and handed to one executor.
    ///
    /// # Errors
    /// Returns an error when another process owns the schedule.
    pub fn automation_scheduler(&self) -> Result<AutomationScheduler, LocalHostError> {
        let lease =
            ExecutionLease::acquire(&self.config.database.with_file_name(".automations.lock"))
                .map_err(|error| {
                    LocalHostError::InvalidRequest(format!(
                        "another process owns this Host's automation schedule: {error}"
                    ))
                })?;
        Ok(AutomationScheduler {
            database: self.config.database.clone(),
            triggers: Arc::new(TriggerContext::new(self.config.home.path().to_path_buf())),
            _lease: lease,
        })
    }
}

impl AutomationScheduler {
    /// Returns the oldest run without a result, or admits the next due
    /// occurrence. A run keeps its identity until its result is recorded, so a
    /// scheduler that restarts hands the same run on again instead of a new one.
    /// An occurrence too late for its schedule, or one its plugin says is no
    /// longer due, comes back already skipped:
    /// its status is set, it is not to be executed, and the next call moves on.
    /// While `plugin_running`, the caller is running a plugin's run beside
    /// the schedule, and only a run sent to an agent comes back, a reminder's
    /// included, so it fires on time during a long plugin run.
    ///
    /// # Errors
    /// Returns catalog and stored-data failures.
    pub async fn next_run(
        &self,
        now_ms: i64,
        plugin_running: bool,
    ) -> Result<Option<ScheduledRun>, LocalHostError> {
        let database = self.database.clone();
        let context = Arc::clone(&self.triggers);
        tokio::task::spawn_blocking(move || {
            let Some(run) = runs::next(&database, &context, now_ms, plugin_running)? else {
                return Ok(None);
            };
            let db = catalog::open_verified(&database)?;
            let origin_session_id = store::origin(&db, run.automation_id)?;
            // The plugin's error keeps its source, so a busy catalog shows.
            let plugin = triggers::runner(&db, run.automation_id)?.map(HostPluginId::id);
            Ok::<_, LocalHostError>(Some(ScheduledRun {
                run,
                origin_session_id,
                plugin,
            }))
        })
        .await?
    }

    /// Runs one admitted run of a plugin's trigger on `host`, as of
    /// `now_ms`. Every ending the plugin decides comes back to be recorded;
    /// a run that stops part way, by a crash or by dropping this future, is
    /// handed on again by [`Self::next_run`] and resumes.
    ///
    /// # Errors
    ///
    /// Rejects a run no plugin owns.
    pub async fn run_for_plugin(
        &self,
        host: &LocalHost,
        due: &ScheduledRun,
        now_ms: i64,
    ) -> Result<PluginRun, LocalHostError> {
        let plugin = due.plugin.and_then(HostPluginId::parse).ok_or_else(|| {
            LocalHostError::InvalidRequest(
                "an agent's automation run is sent to the agent, not run on the Host".to_owned(),
            )
        })?;
        Ok(PluginRun {
            run: due.run.id,
            ended: triggers::run(host, plugin, &due.run, now_ms).await?,
        })
    }

    /// Records how a plugin's run ended, which ends it.
    ///
    /// # Errors
    ///
    /// Returns a conflict for a run that already has a result.
    pub async fn finish_plugin_run(
        &self,
        finished: PluginRun,
        now_ms: i64,
    ) -> Result<(), LocalHostError> {
        let database = self.database.clone();
        let PluginRun { run, ended } = finished;
        Ok(tokio::task::spawn_blocking(move || {
            runs::end(
                &database,
                run,
                ended.status,
                &ended.output,
                ended.failed_tool_calls,
                now_ms,
            )
        })
        .await??)
    }

    /// Records how an executed run ended, which ends it.
    ///
    /// # Errors
    /// Returns a conflict for a run that is unknown or already has a result,
    /// and rejects a `Skipped` outcome, which only the Host decides.
    pub async fn finish_run(
        &self,
        run: Uuid,
        outcome: RunOutcome,
        now_ms: i64,
    ) -> Result<(), LocalHostError> {
        let database = self.database.clone();
        Ok(
            tokio::task::spawn_blocking(move || runs::finish(&database, run, &outcome, now_ms))
                .await??,
        )
    }

    /// Records that this scheduler is alive, for Host observation.
    ///
    /// # Errors
    /// Returns catalog failures.
    pub async fn heartbeat(&self, now_ms: i64) -> Result<(), LocalHostError> {
        let database = self.database.clone();
        Ok(tokio::task::spawn_blocking(move || runs::heartbeat(&database, now_ms)).await??)
    }

    /// Deleted automations whose own conversation, the RCP task with the
    /// automation's id and the session it executes in, is still to be
    /// deleted. The schedule's owner deletes each, then reports it with
    /// [`Self::conversation_deleted`].
    ///
    /// # Errors
    /// Returns catalog failures.
    pub async fn conversations_to_delete(&self) -> Result<Vec<Uuid>, LocalHostError> {
        let database = self.database.clone();
        Ok(tokio::task::spawn_blocking(move || {
            retention::conversations_to_delete(&catalog::open_verified(&database)?)
        })
        .await??)
    }

    /// Records that `automation`'s conversation is gone from the coordinator
    /// and the node that ran it.
    ///
    /// # Errors
    /// Returns catalog failures.
    pub async fn conversation_deleted(&self, automation: Uuid) -> Result<(), LocalHostError> {
        let database = self.database.clone();
        Ok(tokio::task::spawn_blocking(move || {
            retention::conversation_deleted(&catalog::open_verified(&database)?, automation)
        })
        .await??)
    }
}
