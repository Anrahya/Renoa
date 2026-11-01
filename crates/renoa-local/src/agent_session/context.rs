//! The context points one session command resolves: the session context its
//! system prompt carries, and the trace of contributors a point left out.

use renoa_kernel::CommandId;
use uuid::Uuid;

use super::AgentSession;
use crate::{
    LocalHostError,
    plugins::host::{
        Skipped,
        session_context::{self, ContextRequest, SessionContext},
    },
    trace::TraceRun,
};

impl AgentSession {
    /// The session context `command_id` resolves with when its turn talks to
    /// `principal`; a turn with no person has none.
    pub(super) async fn session_context(
        &self,
        command_id: CommandId,
        principal: Option<Uuid>,
    ) -> Result<(SessionContext, Vec<Skipped>), LocalHostError> {
        let Some(principal) = principal else {
            return Ok((SessionContext::default(), Vec::new()));
        };
        let standing = self.kernel.command_standing(command_id)?;
        let database = self.host.database.clone();
        let data_directory = self.host.home.path().to_path_buf();
        let pin = self.session_context_pin.clone();
        let agent = self.agent;
        // Plugin state, the person's notebook, and the pin are blocking I/O.
        Ok(tokio::task::spawn_blocking(move || {
            session_context::resolve(&ContextRequest {
                database: &database,
                data_directory: &data_directory,
                agent,
                principal,
                command: command_id,
                standing,
                pin: &pin,
            })
        })
        .await?)
    }
}

/// The trace entry and its telemetry fallback for one context point.
pub(super) type SkipTrace = (&'static str, &'static str);
pub(super) const MESSAGE_CONTEXT_TRACE: SkipTrace =
    ("message_context_skipped", "message_context_untraced");
pub(super) const SESSION_CONTEXT_TRACE: SkipTrace =
    ("session_context_skipped", "session_context_untraced");

/// Traces the contributors one context point left out of `command_id`. A trace
/// failure never costs the turn its outcome.
pub(super) async fn record_skipped(
    trace: &TraceRun,
    (kind, untraced): SkipTrace,
    command_id: CommandId,
    skipped: &[Skipped],
) {
    if skipped.is_empty() {
        return;
    }
    if let Err(error) = trace
        .record_host(
            kind,
            Some("degraded"),
            serde_json::json!({ "command_id": command_id, "skipped": skipped }),
        )
        .await
    {
        renoa_telemetry::event(
            "renoa.host",
            "warn",
            untraced,
            &serde_json::json!({ "command_id": command_id, "error": error.to_string() }),
        );
    }
}
