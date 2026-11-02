//! What a pass does with a job, or a memory session, that cannot go on, so
//! nothing wedges every later pass.
//!
//! A job whose command could not run stays for the next pass, which resumes
//! it in the same session. Each distinct run that fails it counts once, however
//! often that run tried, a run resumed after a restart included; once
//! [`ATTEMPTS`] runs have, it is closed as abandoned, without an answer, so
//! Mnem never applies or counts it, and the agent moves to a new memory
//! session: the kernel admits nothing else in a session while one command is
//! unfinished. A stored session that cannot open with no job pending counts
//! the same way, on its generation, and moves the agent on after
//! [`ATTEMPTS`] runs. A session never stored, or one whose storage cannot be
//! read, is not moved past: a new one would fail the same way.

use std::sync::Arc;

use serde_json::json;
use uuid::Uuid;

#[cfg(test)]
use super::fault;
use super::{
    super::{jobs, sessions},
    Halt, Pass, receipts,
};
use crate::{AgentSession, LocalHostError, host::catalog};

/// The distinct runs a job's command, or a stored session, may fail before
/// the agent moves on. A run counts once however often it tried, so a run
/// resumed after a restart is not counted again.
pub(crate) const ATTEMPTS: u32 = 3;

impl Pass<'_> {
    /// Opens the agent's memory session. One that cannot open counts against
    /// `pending`, the job a stopped pass left, or with none against the
    /// session when it was stored.
    pub(crate) async fn session(&self, pending: Option<Uuid>) -> Result<Arc<AgentSession>, Halt> {
        let (database, agent) = (self.database.clone(), self.agent);
        let generation = tokio::task::spawn_blocking(move || {
            sessions::current(&catalog::open_verified(&database)?, agent)
        })
        .await??;
        let generation = *self.generation.get_or_init(|| generation);
        let id = sessions::session_id(agent, generation);
        let opened: Result<_, LocalHostError> = async {
            let workspace = self.host.agent_workspace(agent).await?;
            self.host.ensure_agent_session(agent, &workspace, id).await
        }
        .await;
        let error = match opened {
            Ok(session) => {
                let database = self.database.clone();
                tokio::task::spawn_blocking(move || sessions::opened(&database, agent, generation))
                    .await??;
                return Ok(session);
            }
            Err(error) => error,
        };
        let reason = format!("memory session {generation} could not open: {error}");
        Err(match (pending, self.host.has_session(id)) {
            (Some(_), _) => self.stalled(pending, &reason).await,
            (None, Ok(true)) => self.stalled(None, &reason).await,
            (None, Ok(false)) => Halt::Failed(reason),
            (None, Err(unread)) => Halt::Failed(format!(
                "{reason}; whether it was stored could not be read: {unread}"
            )),
        })
    }

    /// Counts this run against the job of `command`, or with none the
    /// session it opened, and moves the agent on once [`ATTEMPTS`] runs have
    /// failed it. Returns why the pass stops.
    pub(super) async fn stalled(&self, command: Option<Uuid>, reason: &str) -> Halt {
        let Some(&observed) = self.generation.get() else {
            return Halt::Failed(format!("{reason}; no memory session was opened"));
        };
        let (database, agent, run, now_ms) =
            (self.database.clone(), self.agent, self.run, self.now_ms);
        let recorded = tokio::task::spawn_blocking({
            let reason = reason.to_owned();
            move || {
                let tries = match command {
                    Some(command) => jobs::failed_attempt(&database, command, run)?,
                    None => sessions::failed_open(&database, agent, observed, run)?,
                };
                #[cfg(test)]
                if fault::hit(run, fault::Point::Counted).is_err() {
                    return Ok(Halt::Crashed);
                }
                if tries < ATTEMPTS {
                    return Ok(Halt::Failed(format!(
                        "{reason} ({tries} of {ATTEMPTS} runs); the next run tries again"
                    )));
                }
                let why = match command {
                    Some(command) => {
                        format!("memory job {command} could not run on {ATTEMPTS} runs: {reason}")
                    }
                    None => format!("{reason} (on {ATTEMPTS} runs)"),
                };
                let generation =
                    sessions::move_on(&database, agent, observed, command, &why, now_ms)?;
                receipts::event(
                    "warn",
                    "memory_session_moved",
                    &json!({
                        "agent_id": agent,
                        "run_id": run,
                        "generation": generation,
                        "abandoned_command_id": command,
                        "reason": why,
                    }),
                );
                Ok::<_, catalog::HostCatalogError>(Halt::Failed(format!(
                    "{why}; the next run uses memory session {generation}"
                )))
            }
        })
        .await;
        match recorded {
            Ok(Ok(halt)) => halt,
            Ok(Err(error)) => {
                Halt::Failed(format!("{reason}, and it could not be recorded: {error}"))
            }
            Err(error) => Halt::Failed(format!("{reason}, and it could not be recorded: {error}")),
        }
    }
}
