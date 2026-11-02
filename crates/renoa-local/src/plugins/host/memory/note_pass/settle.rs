//! How a pass's command settles: its outcome and the notes it wrote, read
//! from its own messages, recorded once with the notebook's length after it,
//! and traced by counts, never by what was noted.

use std::time::Instant;

use serde_json::json;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use super::{
    Kind, SessionRef, catalog_call, event, input, now_ms, receipt,
    store::{self, Outcome},
    unavailable,
};
use crate::{AgentSession, LocalTurnOutcome, plugins::PluginError};

/// Closes the pass a stopped process left in `session`, if any, without
/// resuming it: its command is cancelled, so a model call is not made again
/// and a note whose call had begun is not written twice. It counts only when
/// its command had already completed.
///
/// # Errors
///
/// Returns catalog, notebook and execution failures; the pass then stays to
/// be closed by the next message or run.
pub(crate) async fn close_stale(session: &AgentSession) -> Result<(), PluginError> {
    let at = session.pass_ref();
    let id = at.session;
    let Some(frozen) =
        catalog_call(&at.database, move |db| Ok(store::unsettled_in(db, id)?)).await?
    else {
        return Ok(());
    };
    let started = Instant::now();
    let cancelled = CancellationToken::new();
    cancelled.cancel();
    let outcome = session
        .execute_note_pass(frozen.command, &frozen.prompt, frozen.principal, cancelled)
        .await
        .map_err(|error| unavailable(&error))?;
    let ran = Ran {
        at: &at,
        command: frozen.command,
        principal: frozen.principal,
        run: None,
        kind: None,
        started,
    };
    let settled = ran.settle(session, &outcome, now_ms(&at.database)).await?;
    if settled.outcome == Outcome::Cancelled {
        ran.cancelled("a stopped process left it");
    }
    Ok(())
}

/// A pass's command once it ran, for settling and telemetry.
pub(super) struct Ran<'a> {
    pub(super) at: &'a SessionRef,
    pub(super) command: Uuid,
    pub(super) principal: Uuid,
    pub(super) run: Option<Uuid>,
    pub(super) kind: Option<Kind>,
    pub(super) started: Instant,
}

/// How one pass settled.
pub(super) struct Settled {
    pub(super) outcome: Outcome,
    pub(super) noted: usize,
    pub(super) failed_calls: usize,
}

impl Ran<'_> {
    /// Records how the pass ended from its command's outcome and messages,
    /// and the notebook's length after it, once.
    pub(super) async fn settle(
        &self,
        session: &AgentSession,
        outcome: &LocalTurnOutcome,
        now_ms: i64,
    ) -> Result<Settled, PluginError> {
        let messages = session
            .command_messages(self.command)
            .map_err(|error| unavailable(&error))?;
        let settled = Settled {
            outcome: match outcome {
                LocalTurnOutcome::Completed {
                    stop_reason: renoa_agent::StopReason::Stop,
                    ..
                } => Outcome::Completed,
                LocalTurnOutcome::Cancelled => Outcome::Cancelled,
                LocalTurnOutcome::Completed { .. }
                | LocalTurnOutcome::Failed { .. }
                | LocalTurnOutcome::WaitingForInput
                | LocalTurnOutcome::Compacted { .. } => Outcome::Failed,
            },
            noted: receipt::noted(&messages).len(),
            failed_calls: receipt::failed_calls(&messages),
        };
        let (data_directory, command, principal) =
            (self.at.data_directory.clone(), self.command, self.principal);
        let (outcome_kind, noted) = (settled.outcome, settled.noted);
        catalog_call(&self.at.database, move |db| {
            let length = input::notes(&data_directory, principal)?;
            store::settle(db, command, outcome_kind, noted, length, now_ms)?;
            Ok(())
        })
        .await?;
        let detail = match outcome {
            LocalTurnOutcome::Failed { reason } => Some(reason.as_str()),
            _ => None,
        };
        self.trace(
            settled.outcome.as_str(),
            settled.noted,
            settled.failed_calls,
            detail,
        );
        Ok(settled)
    }

    pub(super) fn trace(
        &self,
        outcome: &str,
        noted: usize,
        failed_calls: usize,
        detail: Option<&str>,
    ) {
        event(
            if outcome == Outcome::Completed.as_str() {
                "info"
            } else {
                "warn"
            },
            "memory_pass",
            &json!({
                "agent_id": self.at.agent,
                "run_id": self.run,
                "command_id": self.command,
                "session_id": self.at.session,
                "principal_id": self.principal,
                "kind": self.kind.map(Kind::as_str),
                "outcome": outcome,
                "noted": noted,
                "failed_tool_calls": failed_calls,
                "duration_ms": u64::try_from(self.started.elapsed().as_millis()).unwrap_or(u64::MAX),
                "detail": detail,
            }),
        );
    }

    pub(super) fn cancelled(&self, reason: &str) {
        event(
            "info",
            "memory_pass_cancelled",
            &json!({
                "agent_id": self.at.agent,
                "run_id": self.run,
                "command_id": self.command,
                "session_id": self.at.session,
                "principal_id": self.principal,
                "reason": reason,
            }),
        );
    }
}
