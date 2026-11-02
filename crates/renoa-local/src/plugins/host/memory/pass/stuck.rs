//! What a pass does with a job, or a memory session, that cannot go on, so
//! nothing wedges every later pass.
//!
//! A job whose command could not run is counted once per pass that tried it
//! and stays for the next, which resumes it in the same session. After
//! [`ATTEMPTS`] it is closed as abandoned, without an answer, so Mnem never
//! applies or counts it, and the agent moves to a new memory session: the
//! kernel admits nothing else in a session while one command is unfinished.
//! A stored session that cannot open with no job pending moves the agent on
//! at once, since nothing in it is unfinished.

use std::sync::Arc;

use serde_json::json;
use uuid::Uuid;

use super::{
    super::{PLUGIN_ID, jobs, sessions},
    Halt, Pass,
};
use crate::{AgentSession, LocalHostError, host::catalog};

/// The passes a job's command may fail before it is abandoned: three nights.
pub(crate) const ATTEMPTS: u32 = 3;

impl Pass<'_> {
    /// Opens the agent's memory session. One that cannot open counts against
    /// `pending`, the job a stopped pass left, or with none moves the agent on
    /// when the session was stored.
    pub(crate) async fn session(&self, pending: Option<Uuid>) -> Result<Arc<AgentSession>, Halt> {
        let (database, agent) = (self.database.clone(), self.agent);
        let generation = tokio::task::spawn_blocking(move || {
            sessions::current(&catalog::open_verified(&database)?, agent)
        })
        .await??;
        let id = sessions::session_id(agent, generation);
        let opened: Result<_, LocalHostError> = async {
            let workspace = self.host.agent_workspace(agent).await?;
            self.host.ensure_agent_session(agent, &workspace, id).await
        }
        .await;
        let error = match opened {
            Ok(session) => return Ok(session),
            Err(error) => error,
        };
        let reason = format!("memory session {generation} could not open: {error}");
        Err(match pending {
            Some(_) => self.stalled(pending, &reason).await,
            None if self.host.has_session(id).unwrap_or(true) => self.stalled(None, &reason).await,
            None => Halt::Failed(reason),
        })
    }

    /// Records that the job of `command`, or with none the session itself,
    /// could not go on, and returns why the pass stops.
    pub(super) async fn stalled(&self, command: Option<Uuid>, reason: &str) -> Halt {
        let (database, agent, now_ms) = (self.database.clone(), self.agent, self.now_ms);
        let reason = reason.to_owned();
        let recorded = tokio::task::spawn_blocking({
            let reason = reason.clone();
            move || {
                let abandoning = match command {
                    Some(command) => {
                        let attempts = jobs::failed_attempt(&database, command)?;
                        if attempts < ATTEMPTS {
                            return Ok(Halt::Failed(format!(
                                "{reason} ({attempts} of {ATTEMPTS} tries); the next run resumes it"
                            )));
                        }
                        Some(command)
                    }
                    None => None,
                };
                let why = match abandoning {
                    Some(command) => {
                        format!("memory job {command} could not run on {ATTEMPTS} runs: {reason}")
                    }
                    None => reason.clone(),
                };
                let generation = sessions::move_on(&database, agent, abandoning, &why, now_ms)?;
                renoa_telemetry::event(
                    PLUGIN_ID,
                    "warn",
                    "memory_session_moved",
                    &json!({
                        "agent_id": agent,
                        "generation": generation,
                        "abandoned_command_id": abandoning,
                        "reason": why,
                    }),
                );
                Ok::<_, crate::host::catalog::HostCatalogError>(Halt::Failed(format!(
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
