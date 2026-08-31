use std::sync::Arc;

use renoa_agent::{AgentEventSink, ContentBlock};
use renoa_kernel::CommandId;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use super::AgentSession;
use crate::{
    LocalHostError, LocalTurnOutcome, LocalWorkspace, ModelChoice, TurnObservation,
    agent_trace::finish_trace,
    host::{RuntimeRequest, resolve_runtime},
    trace::{ObservedEventSink, TraceRun},
};

enum SessionCommand {
    Prompt {
        content: Vec<ContentBlock>,
        observation: TurnObservation,
    },
    Compact,
}

impl SessionCommand {
    const fn name(&self) -> &'static str {
        match self {
            Self::Prompt { .. } => "prompt",
            Self::Compact => "compact",
        }
    }
}

struct TracedTurn<'a> {
    command_id: CommandId,
    command: SessionCommand,
    cancellation: CancellationToken,
    model: ModelChoice,
    reasoning: crate::ReasoningLevel,
    events: Arc<dyn AgentEventSink>,
    trace: &'a TraceRun,
}

impl AgentSession {
    /// Runs one caller-identified prompt through fresh profile composition.
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
        self.execute_turn_observed(request_id, content, TurnObservation::now()?, events)
            .await
    }

    /// Runs one caller-identified prompt with the surface's durable receive time.
    ///
    /// Queue-backed surfaces should use this so restarts and delivery delays do
    /// not change the time observed by the model.
    ///
    /// # Errors
    ///
    /// Returns request coordination, runtime resolution, admission, or execution failures.
    pub async fn execute_turn_observed(
        &self,
        request_id: Uuid,
        content: Vec<ContentBlock>,
        observation: TurnObservation,
        events: Arc<dyn AgentEventSink>,
    ) -> Result<LocalTurnOutcome, LocalHostError> {
        self.execute(
            request_id,
            SessionCommand::Prompt {
                content,
                observation,
            },
            events,
        )
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
        let (guard, cancellation, model, reasoning) = self.begin_prompt(request_id)?;
        let command_id = CommandId::from_uuid(request_id);
        let compact_trace = [ContentBlock::text("/compact")];
        let trace_content = match &command {
            SessionCommand::Prompt { content, .. } => content.as_slice(),
            SessionCommand::Compact => compact_trace.as_slice(),
        };
        let trace = self
            .trace
            .start_run(
                command_id,
                trace_content,
                model.provider().as_str(),
                model.id(),
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
                model,
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
            model,
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
                    "provider": model.provider().as_str(),
                    "model": model.id(),
                    "reasoning": reasoning.as_str()
                }),
            )
            .await?;
        let replay = match &command {
            SessionCommand::Prompt { content, .. } => {
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
        let profile = self.profile()?.clone();
        let runtime = resolve_runtime(
            &self.host,
            RuntimeRequest {
                profile: &profile,
                session_id: renoa_kernel::SessionId::from_uuid(self.id),
                command_id: Some(command_id),
                model: &model,
                reasoning,
                workspace: &workspace,
                events: Some(events),
            },
        )
        .await?;
        match command {
            SessionCommand::Prompt {
                content,
                observation,
            } if profile.uses_turn_timing() => Ok(self
                .kernel
                .execute_observed_turn(command_id, content, observation, &runtime, cancellation)
                .await?),
            SessionCommand::Prompt { content, .. } => Ok(self
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
