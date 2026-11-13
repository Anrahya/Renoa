//! The context points one session command resolves: the session context its
//! system prompt carries, the runtime it resolves with, and the trace of
//! contributors a point left out.

use std::{collections::HashSet, sync::Arc};

use renoa_agent::AgentEventSink;
use renoa_kernel::{CommandId, Runtime};
use uuid::Uuid;

use super::AgentSession;
use crate::{
    LocalHostError, LocalWorkspace, ModelChoice,
    host::{RuntimeRequest, resolve_runtime, synthetic_activity_tools_for_definition},
    plugins::host::{
        Skipped,
        session_context::{self, ContextRequest, SessionContext},
    },
    speaker::ToolAccess,
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

    /// The runtime a command of the agent's conversation runs with: its
    /// definition, with the person it talks to and their session context, and
    /// the tools `tools` grants.
    pub(super) async fn conversation_runtime(
        &self,
        command_id: CommandId,
        principal: Option<Uuid>,
        tools: ToolAccess,
        model: &ModelChoice,
        reasoning: crate::ReasoningLevel,
        events: Arc<dyn AgentEventSink>,
    ) -> Result<(Runtime, Vec<Skipped>), LocalHostError> {
        let workspace = LocalWorkspace::open(&self.workspace)?;
        let (session_context, context_skipped) =
            self.session_context(command_id, principal).await?;
        let definition = self
            .definition()
            .await?
            .with_principal(principal)
            .with_session_context(session_context);
        let runtime = resolve_runtime(
            &self.host,
            RuntimeRequest {
                definition: &definition,
                session_id: renoa_kernel::SessionId::from_uuid(self.id),
                command_id: Some(command_id),
                model,
                reasoning,
                workspace: &workspace,
                events: Some(events),
                tools,
            },
        )
        .await?;
        Ok((runtime, context_skipped))
    }

    /// Returns the tool names configured for this session that receive Renoa's
    /// synthetic `activity` property in their model-visible schemas.
    ///
    /// # Errors
    ///
    /// Returns workspace or agent-definition loading failures.
    pub async fn synthetic_activity_tools(&self) -> Result<HashSet<String>, LocalHostError> {
        let workspace = LocalWorkspace::open(&self.workspace)?;
        let definition = self.definition().await?;
        synthetic_activity_tools_for_definition(
            &self.host,
            &definition,
            &workspace,
            renoa_kernel::SessionId::from_uuid(self.id),
        )
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
