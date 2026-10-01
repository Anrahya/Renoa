use std::sync::Arc;

use renoa_agent::{AgentEventSink, ContentBlock};
use renoa_agent_loop::AgentCommand;
use renoa_kernel::{CancellationId, CommandId};
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use super::AgentSession;
use crate::{
    LocalHostError, LocalTurnOutcome, LocalWorkspace, ModelChoice, Speaker, TurnObservation,
    agent_trace::finish_trace,
    host::{RuntimeRequest, resolve_runtime},
    plugins::host::message_context,
    session::PromptAdmission,
    speaker::ToolAccess,
    trace::{ObservedEventSink, TraceRun},
};

enum SessionCommand {
    Prompt(Prompt),
    Compact,
}

/// One user message, with when and where it was received and who sent it.
struct Prompt {
    content: Vec<ContentBlock>,
    observation: TurnObservation,
    speaker: Speaker,
}

impl SessionCommand {
    /// The person this turn talks to, whose `USER.md` it reads and may edit.
    const fn principal(&self) -> Option<Uuid> {
        match self {
            Self::Prompt(prompt) => prompt.speaker.principal(),
            Self::Compact => None,
        }
    }

    const fn tool_access(&self) -> ToolAccess {
        match self {
            Self::Prompt(prompt) => prompt.speaker.tool_access(),
            Self::Compact => ToolAccess::Granted,
        }
    }

    fn content(&self) -> Option<&[ContentBlock]> {
        match self {
            Self::Prompt(prompt) => Some(&prompt.content),
            Self::Compact => None,
        }
    }

    const fn name(&self) -> &'static str {
        match self {
            Self::Prompt(_) => "prompt",
            Self::Compact => "compact",
        }
    }
}

struct TracedTurn<'a> {
    request_id: Uuid,
    command: SessionCommand,
    cancellation: CancellationToken,
    model: ModelChoice,
    reasoning: crate::ReasoningLevel,
    events: Arc<dyn AgentEventSink>,
    trace: &'a TraceRun,
}

impl AgentSession {
    /// Runs one caller-identified prompt through fresh definition composition.
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
        self.execute_turn_observed_with_cancellation(
            request_id,
            content,
            observation,
            events,
            CancellationToken::new(),
            Speaker::Unidentified,
        )
        .await
    }

    /// Runs a prompt with cancellation owned by its admitting surface.
    ///
    /// The token may be cancelled before startup and remains the active turn's
    /// token until settlement. The caller must durably retain pre-start cancellation.
    /// `speaker` decides the `USER.md` the turn reads and may edit, the plugin
    /// context it is admitted with, and whether its tool calls run.
    ///
    /// # Errors
    ///
    /// Returns request coordination, runtime resolution, admission, or execution failures.
    pub async fn execute_turn_observed_with_cancellation(
        &self,
        request_id: Uuid,
        content: Vec<ContentBlock>,
        observation: TurnObservation,
        events: Arc<dyn AgentEventSink>,
        cancellation: CancellationToken,
        speaker: Speaker,
    ) -> Result<LocalTurnOutcome, LocalHostError> {
        self.execute(
            request_id,
            SessionCommand::Prompt(Prompt {
                content,
                observation,
                speaker,
            }),
            events,
            cancellation,
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
        self.execute_compaction_with_cancellation(request_id, events, CancellationToken::new())
            .await
    }

    /// Runs compaction with a token owned by the admitting surface, including startup.
    ///
    /// # Errors
    ///
    /// Returns request coordination, runtime resolution, admission, or execution failures.
    pub async fn execute_compaction_with_cancellation(
        &self,
        request_id: Uuid,
        events: Arc<dyn AgentEventSink>,
        cancellation: CancellationToken,
    ) -> Result<LocalTurnOutcome, LocalHostError> {
        self.execute(request_id, SessionCommand::Compact, events, cancellation)
            .await
    }

    /// Cancels an idle surface request without resolving execution dependencies.
    ///
    /// `content` is `None` for explicit compaction. The caller must durably retain
    /// cancellation: an absent request returns `Cancelled` without kernel admission.
    /// Existing outcomes replay honestly; unfinished requests receive a durable
    /// cancellation intent and return `None` until their bound runtime settles.
    ///
    /// # Errors
    ///
    /// Returns concurrent activity, request identity, ownership, or storage failures.
    pub fn cancel_before_execution(
        &self,
        request_id: Uuid,
        content: Option<&[ContentBlock]>,
    ) -> Result<Option<LocalTurnOutcome>, LocalHostError> {
        let cancellation = CancellationToken::new();
        cancellation.cancel();
        let (_guard, ..) = self.begin_prompt(request_id, cancellation)?;
        Ok(self.kernel.cancel_before_execution(
            CommandId::from_uuid(request_id),
            content,
            CancellationId::from_uuid(request_id),
        )?)
    }

    /// Returns the newest durable provider usage or post-compaction estimate.
    ///
    /// # Errors
    ///
    /// Returns a kernel journal or persisted-payload failure.
    pub fn latest_context_tokens(&self) -> Result<Option<u64>, LocalHostError> {
        Ok(self.kernel.latest_context_tokens()?)
    }

    fn cancelled_outcome(
        &self,
        request_id: Uuid,
        command: &SessionCommand,
        cancellation: &CancellationToken,
    ) -> Result<Option<LocalTurnOutcome>, LocalHostError> {
        if !cancellation.is_cancelled() {
            return Ok(None);
        }
        Ok(self.kernel.cancel_before_execution(
            CommandId::from_uuid(request_id),
            command.content(),
            CancellationId::from_uuid(request_id),
        )?)
    }

    async fn execute(
        &self,
        request_id: Uuid,
        command: SessionCommand,
        events: Arc<dyn AgentEventSink>,
        cancellation: CancellationToken,
    ) -> Result<LocalTurnOutcome, LocalHostError> {
        let (guard, cancellation, model, reasoning) =
            self.begin_prompt(request_id, cancellation)?;
        let command_id = CommandId::from_uuid(request_id);
        if let Some(outcome) = self.cancelled_outcome(request_id, &command, &cancellation)? {
            return Ok(outcome);
        }
        let compact_trace = [ContentBlock::text("/compact")];
        let trace_content = match &command {
            SessionCommand::Prompt(prompt) => prompt.content.as_slice(),
            SessionCommand::Compact => compact_trace.as_slice(),
        };
        let trace = match self
            .trace
            .start_run(
                command_id,
                trace_content,
                model.provider().as_str(),
                model.id(),
                reasoning.as_str(),
            )
            .await
        {
            Ok(trace) => trace,
            Err(error) => {
                if let Some(outcome) =
                    self.cancelled_outcome(request_id, &command, &cancellation)?
                {
                    return Ok(outcome);
                }
                return Err(error.into());
            }
        };
        let observed: Arc<dyn AgentEventSink> =
            Arc::new(ObservedEventSink::new(Arc::clone(&trace), events));
        let result = self
            .execute_traced_turn(TracedTurn {
                request_id,
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
            request_id,
            command,
            cancellation,
            model,
            reasoning,
            events,
            trace,
        } = turn;
        let command_id = CommandId::from_uuid(request_id);
        let started = trace
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
            .await;
        if let Err(error) = started {
            return self
                .cancelled_outcome(request_id, &command, &cancellation)?
                .ok_or_else(|| error.into());
        }
        let replay = match &command {
            SessionCommand::Prompt(prompt) => self
                .kernel
                .replay_settled_turn(command_id, &prompt.content)?,
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
        if let Some(outcome) = self.cancelled_outcome(request_id, &command, &cancellation)? {
            return Ok(outcome);
        }
        let resolved = async {
            let workspace = LocalWorkspace::open(&self.workspace)?;
            let definition = self.definition().await?.with_principal(command.principal());
            resolve_runtime(
                &self.host,
                RuntimeRequest {
                    definition: &definition,
                    session_id: renoa_kernel::SessionId::from_uuid(self.id),
                    command_id: Some(command_id),
                    model: &model,
                    reasoning,
                    workspace: &workspace,
                    events: Some(events),
                    tools: command.tool_access(),
                },
            )
            .await
        }
        .await;
        let runtime = match resolved {
            Ok(runtime) => runtime,
            Err(error) => {
                if let Some(outcome) =
                    self.cancelled_outcome(request_id, &command, &cancellation)?
                {
                    return Ok(outcome);
                }
                return Err(error);
            }
        };
        match command {
            SessionCommand::Prompt(prompt) => {
                self.admit_prompt(command_id, prompt, &runtime, cancellation, trace)
                    .await
            }
            SessionCommand::Compact => Ok(self
                .kernel
                .execute_compaction(command_id, &runtime, cancellation)
                .await?),
        }
    }

    /// Admits a prompt with the context its surface and enabled plugins
    /// contribute, or resumes the one already admitted under `command_id`, and
    /// drives it.
    async fn admit_prompt(
        &self,
        command_id: CommandId,
        prompt: Prompt,
        runtime: &renoa_kernel::Runtime,
        cancellation: CancellationToken,
        trace: &TraceRun,
    ) -> Result<LocalTurnOutcome, LocalHostError> {
        let Prompt {
            content,
            observation,
            speaker,
        } = prompt;
        let (prompt, skipped) = match self.kernel.prompt_admission(command_id, &content)? {
            PromptAdmission::Admitted(command) => (command, Vec::new()),
            PromptAdmission::New {
                previous_observed_at,
            } => {
                let database = self.host.database.clone();
                let agent = self.agent;
                let observed_at = observation.unix_milliseconds();
                // Reading plugin state is blocking catalog I/O.
                let (entries, skipped) = tokio::task::spawn_blocking(move || {
                    message_context::admit(
                        &database,
                        agent,
                        speaker,
                        observation.surface_context(),
                        observed_at,
                        previous_observed_at,
                    )
                })
                .await?;
                let command = AgentCommand::observed(content, observed_at, entries)
                    .map_err(crate::LocalSessionError::from)?;
                (command, skipped)
            }
        };
        let outcome = self
            .kernel
            .execute_prompt(command_id, prompt, runtime, cancellation)
            .await?;
        // Recorded once the message is admitted; a trace failure never costs
        // the turn its outcome.
        if !skipped.is_empty()
            && let Err(error) = trace
                .record_host(
                    "message_context_skipped",
                    Some("degraded"),
                    serde_json::json!({ "command_id": command_id, "skipped": skipped }),
                )
                .await
        {
            renoa_telemetry::event(
                "renoa.host",
                "warn",
                "message_context_untraced",
                &serde_json::json!({ "command_id": command_id, "error": error.to_string() }),
            );
        }
        Ok(outcome)
    }
}
