//! `renoa.memory`'s nightly run: at 04:00 in the notebook zone, the Host's
//! main agent compacts every person's notebook, then dreams over it.
//!
//! The run is this plugin's trigger (see `triggers`), for the one agent that
//! drives every notebook: the main agent, while `renoa.memory` is on for it
//! ([`driver`]). It is a pass of memory jobs (see `pass`). A notebook that
//! fails stops its own jobs, and the run goes on to the next; a failed
//! compaction still lets that person's Dream run. Dream reads the notes after
//! its watermark, with an overview of the notebook that includes summaries,
//! so missing summaries only thin the overview.

use std::sync::atomic::{AtomicUsize, Ordering};

use jiff::{Timestamp, civil::DateTime};
use mnem::dream::{DreamJob, Trigger};
use uuid::Uuid;

use super::{
    for_jobs,
    jobs::{self, JobId, JobKind, Receipt},
    local_time, notebook_zone,
    pass::{Halt, Pass, summary, unopened},
};
use crate::{
    AgentSession, AutomationRun, LocalHost, LocalHostError, RunStatus,
    documents::{people, read_user_profile},
    host::catalog,
    plugins::host::triggers::Ended,
};

mod trigger;

pub(crate) use trigger::{driver, wanted};

/// The most Dream turns one run spends on one notebook. A job goes stale only
/// when another dream moved the watermark first, or the notebook shrank below
/// the job's end, so a third stale job waits for the next night.
pub(crate) const DREAM_TURNS: usize = 3;

/// Runs one admitted nightly run of `run`'s agent as of `now_ms`, which
/// every job it freezes is dated with.
///
/// # Errors
///
/// Only an injected crash in tests; every other ending is recorded.
pub(crate) async fn run(
    host: &LocalHost,
    run: &AutomationRun,
    now_ms: i64,
) -> Result<Ended, LocalHostError> {
    let night = Night {
        pass: Pass::new(host, run.agent_id, run.id, now_ms),
        bounded: AtomicUsize::new(0),
    };
    let halt = night.work().await.err();
    #[cfg(test)]
    if matches!(halt, Some(Halt::Crashed)) {
        return Err(LocalHostError::InvalidRequest(
            "injected crash in a nightly memory run".to_owned(),
        ));
    }
    let (database, run) = (night.pass.database.clone(), night.pass.run);
    let done = tokio::task::spawn_blocking(move || jobs::of_run(&database, run))
        .await?
        .map_err(LocalHostError::from);
    let (done, summary_error) = match done {
        Ok(done) => (done, None),
        Err(error) => (Vec::new(), Some(error.to_string())),
    };
    let halt = halt.or(summary_error.map(Halt::Failed));
    let status = match &halt {
        None | Some(Halt::Busy) => RunStatus::Succeeded,
        Some(Halt::Off) if done.is_empty() => RunStatus::Skipped,
        Some(Halt::Off) => RunStatus::Succeeded,
        Some(Halt::Notebook(_) | Halt::Unopened(_) | Halt::Stalled(_) | Halt::Failed(_)) => {
            RunStatus::Failed
        }
        #[cfg(test)]
        Some(Halt::Crashed) => RunStatus::Failed,
    };
    Ok(Ended {
        status,
        output: summary(&done, night.bounded.load(Ordering::Relaxed), halt.as_ref()),
        // A memory turn has no tools, so none can fail.
        failed_tool_calls: (status != RunStatus::Skipped).then_some(0),
    })
}

struct Night<'a> {
    pass: Pass<'a>,
    /// The notebooks the compaction bound stopped with summaries still due.
    bounded: AtomicUsize,
}

impl Night<'_> {
    async fn work(&self) -> Result<(), Halt> {
        let pass = &self.pass;
        pass.gate().await?;
        let mut failed = Vec::new();
        let mut pending = pass.unsettled().await?;
        // A Dream answer whose apply began needs no session to be closed.
        if let Some(interrupted) = pending.take_if(|pending| pending.applying) {
            notebook(
                pass.close_interrupted(interrupted).await.map(drop),
                &mut failed,
            )?;
        }
        let session = pass
            .session(pending.as_ref().map(|job| job.command))
            .await?;
        // The kernel admits nothing else in the session while this job's
        // command is unfinished, whichever run froze it.
        if let Some(pending) = pending {
            notebook(pass.settle(&session, pending).await.map(drop), &mut failed)?;
        }
        let local_time = self.local_time().await?;
        let data_directory = pass.data_directory.clone();
        for principal in tokio::task::spawn_blocking(move || people(&data_directory)).await?? {
            let compacted = pass.compact(&session, principal, local_time).await;
            let opened = !matches!(compacted, Err(Halt::Unopened(_)));
            if matches!(compacted, Ok(true)) {
                self.bounded.fetch_add(1, Ordering::Relaxed);
            }
            notebook(compacted.map(drop), &mut failed)?;
            if opened {
                notebook(
                    self.dream(&session, principal, local_time).await,
                    &mut failed,
                )?;
            }
        }
        if failed.is_empty() {
            Ok(())
        } else {
            Err(Halt::Failed(failed.join("; ")))
        }
    }

    /// The notebook zone's wall time at the run's time, which Dream reads as
    /// now.
    async fn local_time(&self) -> Result<DateTime, Halt> {
        let database = self.pass.database.clone();
        let zone =
            tokio::task::spawn_blocking(move || notebook_zone(&catalog::open_verified(&database)?))
                .await??;
        let at = Timestamp::from_millisecond(self.pass.now_ms)
            .map_err(|error| Halt::Failed(format!("the run's time is invalid: {error}")))?;
        Ok(local_time(&zone, at))
    }

    /// Dream, once, or again after a stale job, while the bound allows.
    async fn dream(
        &self,
        session: &AgentSession,
        principal: Uuid,
        local_time: DateTime,
    ) -> Result<(), Halt> {
        let pass = &self.pass;
        let dreams: Vec<_> = pass
            .done(principal)
            .await?
            .into_iter()
            .filter(|job| job.kind == JobKind::Dream)
            .collect();
        if dreams
            .last()
            .is_some_and(|job| job.receipt != Some(Receipt::Conflict))
        {
            return Ok(());
        }
        let mut turns = dreams.len();
        while turns < DREAM_TURNS {
            pass.gate().await?;
            let Some(job) = self.dream_job(principal, local_time).await? else {
                return Ok(());
            };
            let pending = pass
                .freeze(principal, JobId::dream(&job.id), job.prompt, local_time)
                .await?;
            turns += 1;
            if pass.settle(session, pending).await?.receipt != Receipt::Conflict {
                return Ok(());
            }
        }
        Ok(())
    }

    /// The nightly dream due on the person's notebook, which reads their
    /// `USER.md` so it does not repeat it.
    async fn dream_job(
        &self,
        principal: Uuid,
        local_time: DateTime,
    ) -> Result<Option<DreamJob>, Halt> {
        let data_directory = self.pass.data_directory.clone();
        tokio::task::spawn_blocking(move || {
            let Some(memory) = for_jobs(&data_directory, principal).map_err(unopened)? else {
                return Ok(None);
            };
            let profile = read_user_profile(&data_directory, principal)
                .map_err(unopened)?
                .content;
            let user = Some(profile.as_str()).filter(|text| !text.trim().is_empty());
            memory
                .dream_job(Trigger::Nightly, user, local_time)
                .map_err(super::pass::refusal)
        })
        .await?
    }
}

/// Records a failure of one person's notebook, once when its compaction and
/// its Dream fail for the same cause, and lets the run go on; any other halt
/// ends the run.
fn notebook(result: Result<(), Halt>, failed: &mut Vec<String>) -> Result<(), Halt> {
    match result {
        Err(Halt::Notebook(reason) | Halt::Unopened(reason)) => {
            if !failed.contains(&reason) {
                failed.push(reason);
            }
            Ok(())
        }
        other => other,
    }
}

#[cfg(test)]
mod tests {
    use super::{Halt, notebook};

    #[test]
    fn one_cause_is_reported_once() {
        let mut failed = Vec::new();
        for reason in [
            "notes.log cannot be read",
            "notes.log cannot be read",
            "busy",
        ] {
            notebook(Err(Halt::Notebook(reason.to_owned())), &mut failed).expect("goes on");
        }
        assert_eq!(failed, ["notes.log cannot be read", "busy"]);
    }
}
