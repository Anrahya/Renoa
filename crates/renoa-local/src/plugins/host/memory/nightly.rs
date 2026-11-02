//! `renoa.memory`'s nightly run: at 04:00 in the notebook zone, the Host's
//! main agent compacts every person's notebook, then dreams over it.
//!
//! The run is this plugin's trigger (see `triggers`), for the one agent that
//! drives every notebook: the main agent, while `renoa.memory` is on for it
//! ([`driver`]). Each job is one command of that agent's memory session, a
//! Host-owned session apart from its conversations. The job's Mnem id, prompt
//! and time are frozen before its command is admitted; the model answers
//! with no tools; and a Host step after the turn applies the answer to the
//! notebook and settles the job with a receipt keyed by its command. A run
//! that stopped part way first settles the job it left: the kernel replays or
//! resumes its command, and Mnem settles a compaction answer applied twice. A
//! Dream answer whose apply began is not applied again (see `jobs`). A
//! notebook that fails stops its own jobs, and the run goes on to the next.
//!
//! Mnem is synchronous and takes file locks, so every call to it runs off the
//! async executor.

use std::{path::PathBuf, time::Instant};

use jiff::{Timestamp, civil::DateTime};
use mnem::{compactor::SummaryJob, dream::Trigger, model::Answer};
use renoa_kernel::AgentId;
use serde_json::json;
use uuid::Uuid;

use super::{
    for_jobs,
    jobs::{self, Applying, Done, Job, JobId, JobKind, Pending, Receipt},
    local_time, notebook_zone,
};
use crate::{
    AgentSession, AutomationRun, LocalHost, LocalHostError, RunStatus,
    documents::{people, read_user_profile},
    host::catalog,
    plugins::host::triggers::Ended,
};

#[cfg(test)]
pub(crate) mod fault;
mod receipts;
mod trigger;

use receipts::{Verdict, answer, refusal, summary};
pub(crate) use trigger::{driver, wanted};

/// The most compaction turns one run spends on one notebook, retries and
/// re-asks included; what is left waits for the next night.
pub(crate) const COMPACTION_TURNS: usize = 32;
/// The most Dream turns one run spends on one notebook: a stale job is asked
/// again, and a notebook moving that fast waits for the next night.
pub(crate) const DREAM_TURNS: usize = 3;

/// The memory session of `agent`, one per agent and apart from its
/// conversations.
pub(crate) fn session_id(agent: AgentId) -> Uuid {
    crate::stable_id::stable_id(&format!("renoa.memory.session.v1:{agent}"))
}

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
        host,
        database: host.database().to_path_buf(),
        data_directory: host.home().path().to_path_buf(),
        agent: run.agent_id,
        run: run.id,
        now_ms,
    };
    let halt = night.work().await.err();
    #[cfg(test)]
    if matches!(halt, Some(Halt::Crashed)) {
        return Err(LocalHostError::InvalidRequest(
            "injected crash in a nightly memory run".to_owned(),
        ));
    }
    let (database, run) = (night.database.clone(), night.run);
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
        Some(Halt::Notebook(_) | Halt::Failed(_)) => RunStatus::Failed,
        #[cfg(test)]
        Some(Halt::Crashed) => RunStatus::Failed,
    };
    Ok(Ended {
        status,
        output: summary(&done, halt.as_ref()),
        // A memory turn has no tools, so none can fail.
        failed_tool_calls: (status != RunStatus::Skipped).then_some(0),
    })
}

/// Why a run ended before its work was done.
#[derive(Debug)]
enum Halt {
    /// `renoa.memory` no longer runs for this agent.
    Off,
    /// Another dream held a notebook.
    Busy,
    /// One person's notebook failed; the run goes on to the next person.
    Notebook(String),
    Failed(String),
    #[cfg(test)]
    Crashed,
}

/// Any other error ends the run failed, with its message.
impl<E: std::fmt::Display> From<E> for Halt {
    fn from(error: E) -> Self {
        Self::Failed(error.to_string())
    }
}

struct Night<'a> {
    host: &'a LocalHost,
    database: PathBuf,
    data_directory: PathBuf,
    agent: AgentId,
    run: Uuid,
    now_ms: i64,
}

/// A settled job: its receipt, and the job Mnem asked for next when a
/// compaction line was too long.
struct Settled {
    receipt: Receipt,
    retry: Option<SummaryJob>,
}

impl Night<'_> {
    async fn work(&self) -> Result<(), Halt> {
        self.gate().await?;
        let workspace = self.host.agent_workspace(self.agent).await?;
        let session = self
            .host
            .ensure_agent_session(self.agent, &workspace, session_id(self.agent))
            .await?;
        // The kernel admits nothing else in the session while this job's
        // command is unfinished, whichever run froze it.
        let (database, agent) = (self.database.clone(), self.agent);
        let mut failed = Vec::new();
        if let Some(pending) =
            tokio::task::spawn_blocking(move || jobs::unsettled(&database, agent)).await??
        {
            notebook(self.settle(&session, pending).await.map(drop), &mut failed)?;
        }
        let local_time = self.local_time().await?;
        let data_directory = self.data_directory.clone();
        for principal in tokio::task::spawn_blocking(move || people(&data_directory)).await?? {
            let person = async {
                self.compact(&session, principal, local_time).await?;
                self.dream(&session, principal, local_time).await
            };
            notebook(person.await, &mut failed)?;
        }
        if failed.is_empty() {
            Ok(())
        } else {
            Err(Halt::Failed(failed.join("; ")))
        }
    }

    /// Refuses to start a job once `renoa.memory` stopped running for this
    /// agent.
    async fn gate(&self) -> Result<(), Halt> {
        let database = self.database.clone();
        let driver =
            tokio::task::spawn_blocking(move || driver(&catalog::open_verified(&database)?))
                .await??;
        if driver == Some(self.agent) {
            Ok(())
        } else {
            Err(Halt::Off)
        }
    }

    /// The notebook zone's wall time at `now_ms`, which Dream reads as now.
    async fn local_time(&self) -> Result<DateTime, Halt> {
        let database = self.database.clone();
        let zone =
            tokio::task::spawn_blocking(move || notebook_zone(&catalog::open_verified(&database)?))
                .await??;
        let at = Timestamp::from_millisecond(self.now_ms)
            .map_err(|error| Halt::Failed(format!("the run's time is invalid: {error}")))?;
        Ok(local_time(&zone, at))
    }

    async fn done(&self, principal: Uuid) -> Result<Vec<Done>, Halt> {
        let (database, run) = (self.database.clone(), self.run);
        let done = tokio::task::spawn_blocking(move || jobs::of_run(&database, run)).await??;
        Ok(done
            .into_iter()
            .filter(|job| job.principal == principal)
            .collect())
    }

    /// Compaction, while Mnem has a summary due and the bound allows. A
    /// notebook whose Dream began in this run is past it.
    async fn compact(
        &self,
        session: &AgentSession,
        principal: Uuid,
        local_time: DateTime,
    ) -> Result<(), Halt> {
        let done = self.done(principal).await?;
        if done.iter().any(|job| job.kind == JobKind::Dream) {
            return Ok(());
        }
        let mut turns = done.len();
        let mut next = None;
        while turns < COMPACTION_TURNS {
            self.gate().await?;
            let job = match next.take() {
                Some(job) => job,
                None => match self.compaction_job(principal).await? {
                    Some(job) => job,
                    None => return Ok(()),
                },
            };
            let SummaryJob { id, prompt } = job;
            let pending = self
                .freeze(principal, JobId::compaction(&id), prompt, local_time)
                .await?;
            turns += 1;
            next = self.settle(session, pending).await?.retry;
        }
        Ok(())
    }

    /// Dream, once, or again after a stale job, while the bound allows.
    async fn dream(
        &self,
        session: &AgentSession,
        principal: Uuid,
        local_time: DateTime,
    ) -> Result<(), Halt> {
        let dreams: Vec<Done> = self
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
            self.gate().await?;
            let Some(job) = self.dream_job(principal, local_time).await? else {
                return Ok(());
            };
            let pending = self
                .freeze(principal, JobId::dream(&job.id), job.prompt, local_time)
                .await?;
            turns += 1;
            if self.settle(session, pending).await?.receipt != Receipt::Conflict {
                return Ok(());
            }
        }
        Ok(())
    }

    async fn compaction_job(&self, principal: Uuid) -> Result<Option<SummaryJob>, Halt> {
        let data_directory = self.data_directory.clone();
        tokio::task::spawn_blocking(move || {
            let Some(memory) = for_jobs(&data_directory, principal).map_err(unreadable)? else {
                return Ok(None);
            };
            memory.compaction_job().map_err(refusal)
        })
        .await?
    }

    /// The nightly dream due on the person's notebook, which reads their
    /// `USER.md` so it does not repeat it.
    async fn dream_job(
        &self,
        principal: Uuid,
        local_time: DateTime,
    ) -> Result<Option<mnem::dream::DreamJob>, Halt> {
        let data_directory = self.data_directory.clone();
        tokio::task::spawn_blocking(move || {
            let Some(memory) = for_jobs(&data_directory, principal).map_err(unreadable)? else {
                return Ok(None);
            };
            let profile = read_user_profile(&data_directory, principal)
                .map_err(unreadable)?
                .content;
            let user = Some(profile.as_str()).filter(|text| !text.trim().is_empty());
            memory
                .dream_job(Trigger::Nightly, user, local_time)
                .map_err(refusal)
        })
        .await?
    }

    async fn freeze(
        &self,
        principal: Uuid,
        id: JobId,
        prompt: String,
        local_time: DateTime,
    ) -> Result<Pending, Halt> {
        let (database, agent, run) = (self.database.clone(), self.agent, self.run);
        let job = Job {
            principal,
            id,
            prompt,
            frozen_at_ms: self.now_ms,
            local_time,
        };
        let pending =
            tokio::task::spawn_blocking(move || jobs::freeze(&database, agent, run, job)).await??;
        #[cfg(test)]
        fault::hit(self.run, fault::Point::Frozen(id.kind()))?;
        Ok(pending)
    }

    /// Runs a frozen job's command, applies its answer, and settles it. A job
    /// whose command could not run stays unsettled for the next run.
    async fn settle(&self, session: &AgentSession, pending: Pending) -> Result<Settled, Halt> {
        let started = Instant::now();
        let Pending {
            command,
            job,
            applying,
        } = pending;
        let kind = job.id.kind();
        let verdict = if applying {
            Ok((Receipt::Interrupted, None, None))
        } else {
            self.answer_and_apply(session, command, &job).await
        };
        let trace = |outcome: &str, halted: bool, detail: Option<&str>| {
            receipts::trace(
                &json!({
                    "agent_id": self.agent,
                    "run_id": self.run,
                    "command_id": command,
                    "principal_id": job.principal,
                    "kind": kind.as_str(),
                    "outcome": outcome,
                    "duration_ms": u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX),
                    "detail": detail,
                }),
                halted,
            );
        };
        let (receipt, retry, detail) = match verdict {
            Ok(verdict) => verdict,
            Err(halt) => {
                trace("unsettled", true, halt.reason());
                return Err(halt);
            }
        };
        let database = self.database.clone();
        let settled =
            tokio::task::spawn_blocking(move || jobs::settle(&database, command, receipt)).await?;
        if let Err(error) = settled {
            trace("unsettled", true, Some(&error.to_string()));
            return Err(error.into());
        }
        let halt = receipts::halt(receipt, kind);
        trace(receipt.as_str(), halt.is_some(), detail.as_deref());
        match halt {
            Some(halt) => Err(halt),
            None => Ok(Settled { receipt, retry }),
        }
    }

    /// The verdict on a job's answer: unanswered, or Mnem's on applying it.
    /// A Dream job is marked before Mnem is called.
    async fn answer_and_apply(
        &self,
        session: &AgentSession,
        command: Uuid,
        job: &Job,
    ) -> Result<Verdict, Halt> {
        let kind = job.id.kind();
        let outcome = session
            .execute_memory_job(command, &job.prompt, job.frozen_at_ms)
            .await
            .map_err(|error| {
                Halt::Failed(format!(
                    "{} job {command} could not run, and the next run resumes it: {error}",
                    kind.as_str()
                ))
            })?;
        let answer = match answer(outcome) {
            Ok(answer) => answer,
            Err(reason) => return Ok((Receipt::Unanswered, None, Some(reason))),
        };
        #[cfg(test)]
        fault::hit(self.run, fault::Point::Answered(kind))?;
        if kind == JobKind::Dream {
            let database = self.database.clone();
            tokio::task::spawn_blocking(move || jobs::mark_applying(&database, command)).await??;
        }
        let applied = self.apply(job, answer).await?;
        #[cfg(test)]
        fault::hit(self.run, fault::Point::Applied(kind))?;
        Ok(applied)
    }

    /// Applies `answer` to the job's notebook.
    async fn apply(&self, job: &Job, answer: Answer) -> Result<Verdict, Halt> {
        let (data_directory, principal, local_time) =
            (self.data_directory.clone(), job.principal, job.local_time);
        let applying = Applying::try_from(job.id)?;
        Ok(tokio::task::spawn_blocking(move || {
            receipts::apply(&data_directory, principal, &applying, answer, local_time)
        })
        .await?)
    }
}

/// Records a failure of one person's notebook and lets the run go on; any
/// other halt ends the run.
fn notebook(result: Result<(), Halt>, failed: &mut Vec<String>) -> Result<(), Halt> {
    match result {
        Err(Halt::Notebook(reason)) => {
            failed.push(reason);
            Ok(())
        }
        other => other,
    }
}

/// A notebook or profile that could not be opened or read.
fn unreadable(error: impl std::fmt::Display) -> Halt {
    Halt::Notebook(error.to_string())
}

impl Halt {
    fn reason(&self) -> Option<&str> {
        match self {
            Self::Notebook(reason) | Self::Failed(reason) => Some(reason),
            Self::Off | Self::Busy => None,
            #[cfg(test)]
            Self::Crashed => Some("injected crash"),
        }
    }
}
