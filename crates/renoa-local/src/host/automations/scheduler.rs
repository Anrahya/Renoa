//! The Host side of automation scheduling: admitting due runs and recording
//! their results. The Host does not execute runs; its scheduler hands each one
//! to an execution node and records the result the node reports.

use std::path::PathBuf;

use uuid::Uuid;

use super::{AutomationError, AutomationRun, LocalHost, LocalHostError, store};
use crate::host::{catalog, lease::ExecutionLease};

/// One admitted run and the conversation it returns to.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ScheduledRun {
    pub run: AutomationRun,
    /// The Host session the automation was created in, when its own agent
    /// created it from one. `None` runs in a conversation of the automation's
    /// own.
    pub origin_session_id: Option<Uuid>,
}

/// Exclusive ownership of a Host's automation schedule. Dropping it releases
/// the schedule to the next owner.
pub struct AutomationScheduler {
    database: PathBuf,
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
            _lease: lease,
        })
    }
}

impl AutomationScheduler {
    /// Returns the oldest run without a result, or admits the next due
    /// occurrence. A run keeps its identity until its result is recorded, so a
    /// scheduler that restarts hands the same run on again instead of a new one.
    ///
    /// # Errors
    /// Returns catalog and stored-data failures.
    pub async fn next_run(&self, now_ms: i64) -> Result<Option<ScheduledRun>, LocalHostError> {
        let database = self.database.clone();
        Ok(tokio::task::spawn_blocking(move || {
            let Some(run) = store::next(&database, now_ms)? else {
                return Ok(None);
            };
            let db = catalog::open_verified(&database)?;
            let origin_session_id = store::origin(&db, run.automation_id)?;
            Ok::<_, AutomationError>(Some(ScheduledRun {
                run,
                origin_session_id,
            }))
        })
        .await??)
    }

    /// Records a run's result, which ends it.
    ///
    /// # Errors
    /// Returns a conflict for a run that is unknown or already has a result.
    pub async fn finish_run(&self, run: Uuid, output: String) -> Result<(), LocalHostError> {
        let database = self.database.clone();
        Ok(tokio::task::spawn_blocking(move || store::finish(&database, run, &output)).await??)
    }
}
