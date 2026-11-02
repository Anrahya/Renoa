//! What a run of passes reports: its output, by outcome and count, and an
//! event for each decision Mnem made. Never what a note or a pass says.

use mnem::pass::PassDecision;
use renoa_kernel::AgentId;
use serde_json::json;
use uuid::Uuid;

use super::{
    Kind, event,
    settle::{Outcome, Settled},
    store::Conversation,
};

/// What a run's passes did, by outcome, never by what they noted.
#[derive(Default)]
pub(super) struct Tally {
    pub(super) completed: usize,
    pub(super) cancelled: usize,
    pub(super) failed: usize,
    pub(super) unsettled: usize,
    pub(super) waiting: usize,
    pub(super) skipped: usize,
    pub(super) busy: usize,
    pub(super) noted: usize,
    pub(super) failed_calls: u32,
    /// The people whose notebooks a pass noted in.
    pub(super) noted_in: Vec<Uuid>,
}

impl Tally {
    /// Counts one settled pass with `principal`.
    pub(super) fn count(&mut self, principal: Uuid, settled: &Settled) {
        match settled.outcome {
            Outcome::Completed => self.completed += 1,
            Outcome::Cancelled => self.cancelled += 1,
            Outcome::Failed => self.failed += 1,
        }
        self.noted += settled.noted;
        self.failed_calls = self
            .failed_calls
            .saturating_add(u32::try_from(settled.failed_calls).unwrap_or(u32::MAX));
        if settled.noted > 0 && !self.noted_in.contains(&principal) {
            self.noted_in.push(principal);
        }
    }

    /// Whether no conversation was visited.
    pub(super) const fn is_empty(&self) -> bool {
        self.completed
            + self.cancelled
            + self.failed
            + self.unsettled
            + self.waiting
            + self.skipped
            + self.busy
            == 0
    }

    /// The run's output.
    pub(super) fn summary(&self) -> String {
        if self.is_empty() {
            return "No conversation was due a memory pass.".to_owned();
        }
        let mut parts = vec![format!(
            "Memory passes: {} completed, {} noted, {} cancelled, {} failed",
            self.completed, self.noted, self.cancelled, self.failed
        )];
        for (count, what) in [
            (self.unsettled, "unsettled"),
            (self.waiting, "waiting"),
            (self.busy, "in a turn"),
            (self.skipped, "skipped"),
        ] {
            if count > 0 {
                parts.push(format!("{count} {what}"));
            }
        }
        format!("{}.", parts.join("; "))
    }
}

/// Logs Mnem's `decision` on a pass of `kind` in `conversation` for the
/// agent's run; `busy` names why one due could not run.
pub(super) fn trace_decision(
    (agent, run, kind): (AgentId, Uuid, Kind),
    conversation: &Conversation,
    decision: PassDecision,
    busy: Option<&str>,
) {
    let (name, reason, until) = match (decision, busy) {
        (_, Some(busy)) => ("wait", Some(busy), None),
        (PassDecision::Run, None) => ("run", None, None),
        (PassDecision::Wait { until }, None) => (
            "wait",
            Some(match kind {
                Kind::Daytime => "not due yet",
                Kind::Sweep => "the person is still talking",
            }),
            Some(until.to_string()),
        ),
        (PassDecision::Skip { reason }, None) => ("skip", Some(reason), None),
    };
    event(
        "info",
        "memory_pass_decision",
        &json!({
            "agent_id": agent,
            "run_id": run,
            "session_id": conversation.session,
            "principal_id": conversation.principal,
            "kind": kind.as_str(),
            "decision": name,
            "reason": reason,
            "until": until,
            "user_chars": conversation.user_chars,
        }),
    );
}

/// Logs, as `name`, what kept one conversation of the agent's run from its
/// pass; the run goes on to the next.
pub(super) fn trouble(
    name: &'static str,
    (agent, run): (AgentId, Uuid),
    session: Uuid,
    error: &dyn std::fmt::Display,
) {
    event(
        "warn",
        name,
        &json!({
            "agent_id": agent,
            "run_id": run,
            "session_id": session,
            "error": error.to_string(),
        }),
    );
}
