//! A pass of `renoa.memory` jobs for one agent and one run, and the
//! compaction any pass runs. The nightly run is one caller.
//!
//! Each job is one command of the agent's memory session, a Host-owned
//! session apart from its conversations. The job's Mnem id, prompt and time
//! are frozen before its command is admitted; the model answers with no
//! tools; and a Host step after the turn applies the answer to the notebook
//! and settles the job with a receipt keyed by its command. A pass first
//! settles the job a stopped one left: the kernel replays or resumes its
//! command, and Mnem settles a compaction answer applied twice. A Dream answer
//! whose apply began is not applied again (see `jobs`). A job or session that
//! cannot go on is bounded (see `stuck`).
//!
//! Mnem is synchronous and takes file locks, so every call to it runs off the
//! async executor.

use std::{path::PathBuf, sync::Arc, time::Instant};

use jiff::civil::DateTime;
use mnem::{compactor::SummaryJob, model::Answer};
use renoa_kernel::AgentId;
use serde_json::json;
use uuid::Uuid;

use super::{
    for_jobs,
    jobs::{self, Applying, Done, Job, JobId, JobKind, Pending, Receipt},
};
use crate::{AgentSession, LocalHost, host::catalog};

#[cfg(test)]
pub(crate) mod fault;
mod receipts;
mod stuck;

#[cfg(test)]
pub(crate) use receipts::EVENTS;
use receipts::{Verdict, answer};
pub(super) use receipts::{event, refusal, summary};

/// The most compaction turns one pass spends on one notebook, retries and
/// re-asks included; what is left waits for a later pass, and the bound is
/// logged.
pub(crate) const COMPACTION_TURNS: usize = 32;

/// Why a pass ended before its work was done.
#[derive(Debug)]
pub(crate) enum Halt {
    /// `renoa.memory` no longer runs for this agent.
    Off,
    /// Another dream held a notebook.
    Busy,
    /// One person's notebook failed a job; the pass goes on.
    Notebook(String),
    /// One person's notebook or profile could not be opened at all.
    Unopened(String),
    /// A job's command could not run; `stuck` counts it.
    Stalled(String),
    Failed(String),
    #[cfg(test)]
    Crashed,
}

/// Any other error ends the pass failed, with its message.
impl<E: std::fmt::Display> From<E> for Halt {
    fn from(error: E) -> Self {
        Self::Failed(error.to_string())
    }
}

impl Halt {
    pub(super) fn reason(&self) -> Option<&str> {
        match self {
            Self::Notebook(reason)
            | Self::Unopened(reason)
            | Self::Stalled(reason)
            | Self::Failed(reason) => Some(reason),
            Self::Off | Self::Busy => None,
            #[cfg(test)]
            Self::Crashed => Some("injected crash"),
        }
    }
}

/// The agent, run and time a pass's jobs are frozen with.
pub(crate) struct Pass<'a> {
    pub(super) host: &'a LocalHost,
    pub(super) database: PathBuf,
    pub(super) data_directory: PathBuf,
    pub(super) agent: AgentId,
    pub(super) run: Uuid,
    pub(super) now_ms: i64,
    /// The memory session generation this pass opened.
    pub(super) generation: std::sync::OnceLock<u32>,
}

/// A settled job: its receipt, and the job Mnem asked for next when a
/// compaction line was too long.
pub(super) struct Settled {
    pub(super) receipt: Receipt,
    retry: Option<SummaryJob>,
}

impl<'a> Pass<'a> {
    pub(crate) fn new(host: &'a LocalHost, agent: AgentId, run: Uuid, now_ms: i64) -> Self {
        Self {
            host,
            database: host.database().to_path_buf(),
            data_directory: host.home().path().to_path_buf(),
            agent,
            run,
            now_ms,
            generation: std::sync::OnceLock::new(),
        }
    }

    /// Refuses to start a job once `renoa.memory` stopped running for this
    /// agent.
    pub(super) async fn gate(&self) -> Result<(), Halt> {
        let database = self.database.clone();
        let driver = tokio::task::spawn_blocking(move || {
            super::nightly::driver(&catalog::open_verified(&database)?)
        })
        .await??;
        if driver == Some(self.agent) {
            Ok(())
        } else {
            Err(Halt::Off)
        }
    }

    /// The jobs of this pass's run for `principal`.
    pub(super) async fn done(&self, principal: Uuid) -> Result<Vec<Done>, Halt> {
        let (database, run) = (self.database.clone(), self.run);
        let done = tokio::task::spawn_blocking(move || jobs::of_run(&database, run)).await??;
        Ok(done
            .into_iter()
            .filter(|job| job.principal == principal)
            .collect())
    }

    /// The memory session, once the job a stopped pass left is settled: the
    /// kernel admits nothing else in the session while that job's command is
    /// unfinished, whichever run froze it. A notebook that job stops is added
    /// to `failed`.
    pub(super) async fn open(&self, failed: &mut Vec<String>) -> Result<Arc<AgentSession>, Halt> {
        let mut pending = self.unsettled().await?;
        // A Dream answer whose apply began needs no session to be closed.
        if let Some(interrupted) = pending.take_if(|pending| pending.applying) {
            notebook(self.close_interrupted(interrupted).await.map(drop), failed)?;
        }
        let session = self
            .session(pending.as_ref().map(|job| job.command))
            .await?;
        if let Some(pending) = pending {
            notebook(self.settle(&session, pending).await.map(drop), failed)?;
        }
        Ok(session)
    }

    /// The agent's job a stopped pass left, if any.
    pub(super) async fn unsettled(&self) -> Result<Option<Pending>, Halt> {
        let (database, agent) = (self.database.clone(), self.agent);
        Ok(tokio::task::spawn_blocking(move || jobs::unsettled(&database, agent)).await??)
    }

    /// Compaction of `principal`'s notebook, while Mnem has a summary due and
    /// the bound allows, and whether the bound stopped it with a summary still
    /// due, which is logged. A notebook whose Dream began in this run is past
    /// it.
    ///
    /// # Errors
    ///
    /// Returns why the pass stopped; [`Halt::Notebook`] and
    /// [`Halt::Unopened`] stop this notebook alone.
    pub(crate) async fn compact(
        &self,
        session: &AgentSession,
        principal: Uuid,
        local_time: DateTime,
    ) -> Result<bool, Halt> {
        let done = self.done(principal).await?;
        if done.iter().any(|job| job.kind == JobKind::Dream) {
            return Ok(false);
        }
        let mut turns = done.len();
        let mut next = None;
        while turns < COMPACTION_TURNS {
            self.gate().await?;
            let job = match next.take() {
                Some(job) => job,
                None => match self.compaction_job(principal).await? {
                    Some(job) => job,
                    None => return Ok(false),
                },
            };
            let SummaryJob { id, prompt } = job;
            let pending = self
                .freeze(principal, JobId::compaction(&id), prompt, local_time)
                .await?;
            turns += 1;
            next = self.settle(session, pending).await?.retry;
        }
        let bounded = next.is_some() || self.compaction_job(principal).await?.is_some();
        if bounded {
            receipts::trace_bound(&json!({
                "agent_id": self.agent,
                "run_id": self.run,
                "principal_id": principal,
                "turns": turns,
            }));
        }
        Ok(bounded)
    }

    /// Compaction of each of `people`'s notebooks at `local_time`, as
    /// [`Self::compact`] does, and the receipts of this pass's run, for its
    /// output. A notebook that fails stops its own compaction alone.
    pub(crate) async fn compact_all(&self, people: &[Uuid], local_time: DateTime) -> String {
        let mut failed = Vec::new();
        let mut bounded = 0;
        let compacted = async {
            self.gate().await?;
            let session = self.open(&mut failed).await?;
            for &principal in people {
                let result = self.compact(&session, principal, local_time).await;
                if matches!(result, Ok(true)) {
                    bounded += 1;
                }
                notebook(result.map(drop), &mut failed)?;
            }
            Ok::<_, Halt>(())
        }
        .await;
        let halt = compacted
            .err()
            .or_else(|| (!failed.is_empty()).then(|| Halt::Failed(failed.join("; "))));
        let (database, run) = (self.database.clone(), self.run);
        match tokio::task::spawn_blocking(move || jobs::of_run(&database, run)).await {
            Ok(Ok(done)) => summary(&done, bounded, halt.as_ref()),
            Ok(Err(error)) => error.to_string(),
            Err(error) => error.to_string(),
        }
    }

    async fn compaction_job(&self, principal: Uuid) -> Result<Option<SummaryJob>, Halt> {
        let data_directory = self.data_directory.clone();
        tokio::task::spawn_blocking(move || {
            let Some(memory) = for_jobs(&data_directory, principal).map_err(unopened)? else {
                return Ok(None);
            };
            memory.compaction_job().map_err(refusal)
        })
        .await?
    }

    pub(super) async fn freeze(
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

    /// Runs a frozen job's command, applies its answer, and settles it.
    pub(super) async fn settle(
        &self,
        session: &AgentSession,
        pending: Pending,
    ) -> Result<Settled, Halt> {
        let started = Instant::now();
        let verdict = self
            .answer_and_apply(session, pending.command, &pending.job)
            .await;
        self.close(pending, verdict, started).await
    }

    /// Closes a Dream job a restart found applying, without its session:
    /// it is never applied again.
    pub(super) async fn close_interrupted(&self, pending: Pending) -> Result<Settled, Halt> {
        let verdict = Ok((Receipt::Interrupted, None, None));
        self.close(pending, verdict, Instant::now()).await
    }

    /// Settles a job with `verdict`, or counts a command that could not run,
    /// and traces it. A job not settled stays for the next pass.
    async fn close(
        &self,
        pending: Pending,
        verdict: Result<Verdict, Halt>,
        started: Instant,
    ) -> Result<Settled, Halt> {
        let Pending { command, job, .. } = pending;
        let kind = job.id.kind();
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
                return Err(match halt {
                    Halt::Stalled(reason) => self.stalled(Some(command), &reason).await,
                    halt => halt,
                });
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
                Halt::Stalled(format!(
                    "{} job {command} could not run: {error}",
                    kind.as_str()
                ))
            })?;
        if matches!(
            outcome,
            crate::LocalTurnOutcome::Completed {
                stop_reason: renoa_agent::StopReason::Length,
                ..
            }
        ) {
            // Three cut Dreams in a row make Mnem give up on those notes, so
            // the output limit wants raising before then.
            receipts::event(
                "warn",
                "memory_turn_cut",
                &json!({
                    "agent_id": self.agent,
                    "run_id": self.run,
                    "command_id": command,
                    "kind": kind.as_str(),
                }),
            );
        }
        let answer = match answer(outcome, kind) {
            Ok(answer) => answer,
            Err((receipt, reason)) => return Ok((receipt, None, Some(reason))),
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

/// Records a failure of one person's notebook, once when its compaction and
/// its Dream fail for the same cause, and lets the run go on; any other halt
/// ends the run.
pub(super) fn notebook(result: Result<(), Halt>, failed: &mut Vec<String>) -> Result<(), Halt> {
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

/// A notebook or profile that could not be opened or read.
pub(super) fn unopened(error: impl std::fmt::Display) -> Halt {
    Halt::Unopened(error.to_string())
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
