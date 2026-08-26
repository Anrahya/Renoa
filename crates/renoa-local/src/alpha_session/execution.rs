use std::sync::Arc;

use renoa_agent::{AgentEventSink, ContentBlock};
use renoa_kernel::CommandId;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use super::AlphaSession;
use crate::{
    LocalHostError, LocalTurnOutcome, LocalWorkspace,
    alpha_trace::finish_trace,
    host::{require_model, resolve_runtime},
    trace::{ObservedEventSink, TraceRun},
};

enum SessionCommand {
    Prompt(Vec<ContentBlock>),
    Compact,
}

impl SessionCommand {
    const fn name(&self) -> &'static str {
        match self {
            Self::Prompt(_) => "prompt",
            Self::Compact => "compact",
        }
    }
}

struct TracedTurn<'a> {
    command_id: CommandId,
    command: SessionCommand,
    cancellation: CancellationToken,
    provider: crate::ModelProvider,
    model_id: &'a str,
    reasoning: crate::ReasoningLevel,
    events: Arc<dyn AgentEventSink>,
    trace: &'a TraceRun,
}

impl AlphaSession {
    /// Runs one caller-identified prompt through fresh Alpha composition.
    ///
    /// Workspace instructions are read for every newly admitted operation.
    /// The resolved behavior then freezes in that operation's kernel manifest.
    ///
    /// # Errors
    ///
    /// Returns request coordination, runtime resolution, admission, or execution failures.
    pub async fn execute_turn(
        &self,
        request_id: Uuid,
        content: Vec<ContentBlock>,
        events: Arc<dyn AgentEventSink>,
    ) -> Result<LocalTurnOutcome, LocalHostError> {
        self.execute(request_id, SessionCommand::Prompt(content), events)
            .await
    }

    /// Runs one caller-identified explicit compaction operation.
    ///
    /// The summary model activity is observable through `events`, while the
    /// durable operation completes without a normal assistant call afterward.
    ///
    /// # Errors
    ///
    /// Returns request coordination, runtime resolution, admission, or execution failures.
    pub async fn execute_compaction(
        &self,
        request_id: Uuid,
        events: Arc<dyn AgentEventSink>,
    ) -> Result<LocalTurnOutcome, LocalHostError> {
        self.execute(request_id, SessionCommand::Compact, events)
            .await
    }

    /// Returns the newest durable provider usage or post-compaction estimate.
    ///
    /// # Errors
    ///
    /// Returns a kernel journal or persisted-payload failure.
    pub fn latest_context_tokens(&self) -> Result<Option<u64>, LocalHostError> {
        Ok(self.kernel.latest_context_tokens()?)
    }

    async fn execute(
        &self,
        request_id: Uuid,
        command: SessionCommand,
        events: Arc<dyn AgentEventSink>,
    ) -> Result<LocalTurnOutcome, LocalHostError> {
        let (guard, cancellation, provider, model_id, reasoning) = self.begin_prompt(request_id)?;
        let command_id = CommandId::from_uuid(request_id);
        let compact_trace = [ContentBlock::text("/compact")];
        let trace_content = match &command {
            SessionCommand::Prompt(content) => content.as_slice(),
            SessionCommand::Compact => compact_trace.as_slice(),
        };
        let trace = self
            .trace
            .start_run(
                command_id,
                trace_content,
                provider.as_str(),
                &model_id,
                reasoning.as_str(),
            )
            .await?;
        let observed: Arc<dyn AgentEventSink> =
            Arc::new(ObservedEventSink::new(Arc::clone(&trace), events));
        let result = self
            .execute_traced_turn(TracedTurn {
                command_id,
                command,
                cancellation,
                provider,
                model_id: &model_id,
                reasoning,
                events: observed,
                trace: &trace,
            })
            .await;
        finish_trace(&trace, &result).await;
        drop(guard);
        result
    }

    async fn execute_traced_turn(
        &self,
        turn: TracedTurn<'_>,
    ) -> Result<LocalTurnOutcome, LocalHostError> {
        let TracedTurn {
            command_id,
            command,
            cancellation,
            provider,
            model_id,
            reasoning,
            events,
            trace,
        } = turn;
        trace
            .record_host(
                "turn_started",
                Some("running"),
                serde_json::json!({
                    "command_id": command_id,
                    "command": command.name(),
                    "provider": provider.as_str(),
                    "model": model_id,
                    "reasoning": reasoning.as_str()
                }),
            )
            .await?;
        let replay = match &command {
            SessionCommand::Prompt(content) => {
                self.kernel.replay_settled_turn(command_id, content)?
            }
            SessionCommand::Compact => self.kernel.replay_settled_compaction(command_id)?,
        };
        if let Some(outcome) = replay {
            trace
                .record_host(
                    "durable_replay",
                    Some("completed"),
                    serde_json::json!({ "command_id": command_id }),
                )
                .await?;
            return Ok(outcome);
        }
        let workspace = LocalWorkspace::open(&self.workspace)?;
        let model = require_model(&self.models, provider, model_id, "active")?;
        let runtime =
            resolve_runtime(&self.host, model, reasoning, &workspace, Some(events)).await?;
        match command {
            SessionCommand::Prompt(content) => Ok(self
                .kernel
                .execute_turn(command_id, content, &runtime, cancellation)
                .await?),
            SessionCommand::Compact => Ok(self
                .kernel
                .execute_compaction(command_id, &runtime, cancellation)
                .await?),
        }
    }
}
