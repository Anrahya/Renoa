use std::sync::Arc;

use renoa_core::{
    CapabilityHost, CommandEnvelope, Message, ModelDriver, ModelError, ResolvedAgent, RunAdmission,
    RunEventKind, RunId, RunStore, StoreError, TerminalState,
};
use thiserror::Error;
use tokio_util::sync::CancellationToken;

use crate::{
    AgentEvent, AgentEventSink,
    events::{append_message, emit_event, finish_message},
};

mod capabilities;
mod model;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EngineConfig {
    pub max_model_rounds: u32,
    pub max_capability_calls_per_response: usize,
}

impl Default for EngineConfig {
    fn default() -> Self {
        Self {
            max_model_rounds: 32,
            max_capability_calls_per_response: 64,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentRunResult {
    pub run_id: RunId,
    pub output: String,
    pub model_rounds: u32,
}

#[derive(Debug, Error)]
pub enum EngineError {
    #[error("run was cancelled")]
    Cancelled,
    #[error("command was already admitted as run {0}")]
    CommandAlreadyAdmitted(RunId),
    #[error("command id conflicts with existing run {0}")]
    CommandConflict(RunId),
    #[error("model returned {actual} capability calls; the per-response limit is {limit}")]
    CapabilityBatchTooLarge { actual: usize, limit: usize },
    #[error("capability task failed: {0}")]
    CapabilityTask(String),
    #[error("model invocation failed: {0}")]
    Model(#[from] ModelError),
    #[error("the model exceeded the configured round limit of {0}")]
    RoundLimit(u32),
    #[error("run storage failed: {0}")]
    Store(#[from] StoreError),
}

pub struct Engine {
    model: Arc<dyn ModelDriver>,
    capabilities: Arc<dyn CapabilityHost>,
    store: Arc<dyn RunStore>,
    config: EngineConfig,
}

impl Engine {
    #[must_use]
    pub fn new(
        model: Arc<dyn ModelDriver>,
        capabilities: Arc<dyn CapabilityHost>,
        store: Arc<dyn RunStore>,
        config: EngineConfig,
    ) -> Self {
        Self {
            model,
            capabilities,
            store,
            config,
        }
    }

    /// Executes one command until the model returns a final response or the run
    /// reaches another terminal condition.
    ///
    /// # Errors
    ///
    /// Returns `EngineError` when command identity conflicts or is already in
    /// progress, the run is cancelled, model or capability execution fails,
    /// configured limits are exceeded, or durable storage fails.
    pub async fn run(
        &self,
        command: CommandEnvelope,
        agent: ResolvedAgent,
        cancellation: CancellationToken,
    ) -> Result<AgentRunResult, EngineError> {
        let mut messages = Vec::new();
        self.run_in_context(command, &agent, &mut messages, cancellation, None)
            .await
    }

    pub(crate) async fn run_in_context(
        &self,
        command: CommandEnvelope,
        agent: &ResolvedAgent,
        messages: &mut Vec<Message>,
        cancellation: CancellationToken,
        event_sink: Option<&dyn AgentEventSink>,
    ) -> Result<AgentRunResult, EngineError> {
        let run_id = match self.store.admit_run(command.clone(), agent.clone()).await? {
            RunAdmission::Admitted(run_id) => run_id,
            RunAdmission::Existing(run_id) => return self.replay_completed(run_id).await,
            RunAdmission::Conflict(run_id) => return Err(EngineError::CommandConflict(run_id)),
        };
        let capabilities = self
            .capabilities
            .specs()
            .into_iter()
            .filter(|capability| agent.capability_grants.contains(&capability.name))
            .collect::<Vec<_>>();
        if self.config.max_model_rounds > 0 {
            emit_event(event_sink, AgentEvent::TurnStart).await;
        }
        let user_message = Message::User {
            text: command.input.text().to_owned(),
        };
        append_message(event_sink, messages, user_message).await;

        for round in 0..self.config.max_model_rounds {
            if round > 0 {
                emit_event(event_sink, AgentEvent::TurnStart).await;
            }
            if cancellation.is_cancelled() {
                emit_event(event_sink, AgentEvent::TurnEnd).await;
                return self.terminal_error(run_id, EngineError::Cancelled).await;
            }

            let model_step = match self
                .model_step(
                    model::build_request(
                        run_id,
                        round,
                        &agent.instructions,
                        messages,
                        &capabilities,
                    ),
                    cancellation.child_token(),
                    event_sink,
                )
                .await
            {
                Ok(model_step) => model_step,
                Err(error) => {
                    emit_event(event_sink, AgentEvent::TurnEnd).await;
                    return self.terminal_error(run_id, error).await;
                }
            };

            let response = model_step.response;
            let assistant_message = Message::Assistant {
                text: response.text.clone(),
                capability_calls: response.capability_calls.clone(),
            };
            finish_message(
                event_sink,
                messages,
                assistant_message,
                model_step.message_started,
            )
            .await;

            if response.capability_calls.is_empty() {
                emit_event(event_sink, AgentEvent::TurnEnd).await;
                return self.complete_run(run_id, response.text, round + 1).await;
            }

            let outcomes = match self
                .capability_batch(
                    run_id,
                    &command,
                    &response,
                    &capabilities,
                    cancellation.child_token(),
                    event_sink,
                )
                .await
            {
                Ok(outcomes) => outcomes,
                Err(error) => {
                    emit_event(event_sink, AgentEvent::TurnEnd).await;
                    return self.terminal_error(run_id, error).await;
                }
            };

            for (request, outcome) in outcomes {
                let capability_message = Message::Capability {
                    call_id: request.call.call_id,
                    name: request.call.name,
                    outcome,
                };
                append_message(event_sink, messages, capability_message).await;
            }
            emit_event(event_sink, AgentEvent::TurnEnd).await;

            if cancellation.is_cancelled() {
                return self.terminal_error(run_id, EngineError::Cancelled).await;
            }
        }

        self.terminal_error(
            run_id,
            EngineError::RoundLimit(self.config.max_model_rounds),
        )
        .await
    }

    async fn complete_run(
        &self,
        run_id: RunId,
        output: String,
        model_rounds: u32,
    ) -> Result<AgentRunResult, EngineError> {
        self.store
            .finish_run(
                run_id,
                TerminalState::Completed {
                    output: output.clone(),
                },
            )
            .await?;
        Ok(AgentRunResult {
            run_id,
            output,
            model_rounds,
        })
    }

    async fn replay_completed(&self, run_id: RunId) -> Result<AgentRunResult, EngineError> {
        let transcript = self.store.load_transcript(run_id).await?;
        let Some(TerminalState::Completed { output }) = transcript.run.terminal else {
            return Err(EngineError::CommandAlreadyAdmitted(run_id));
        };
        let model_rounds = transcript
            .events
            .iter()
            .filter(|event| matches!(event.kind, RunEventKind::ModelRequested { .. }))
            .count();
        let model_rounds = u32::try_from(model_rounds)
            .map_err(|_| StoreError::new("stored model round count exceeds u32"))?;
        Ok(AgentRunResult {
            run_id,
            output,
            model_rounds,
        })
    }

    async fn terminal_error(
        &self,
        run_id: RunId,
        error: EngineError,
    ) -> Result<AgentRunResult, EngineError> {
        match &error {
            EngineError::CommandAlreadyAdmitted(_)
            | EngineError::CommandConflict(_)
            | EngineError::Store(_) => {}
            EngineError::Cancelled => self.cancel_run(run_id).await?,
            EngineError::CapabilityBatchTooLarge { .. }
            | EngineError::CapabilityTask(_)
            | EngineError::Model(_)
            | EngineError::RoundLimit(_) => self.fail_run(run_id, error.to_string()).await?,
        }
        Err(error)
    }

    async fn cancel_run(&self, run_id: RunId) -> Result<(), StoreError> {
        self.store
            .finish_run(
                run_id,
                TerminalState::Cancelled {
                    reason: "caller cancelled the run".to_owned(),
                },
            )
            .await?;
        Ok(())
    }

    async fn fail_run(&self, run_id: RunId, error: String) -> Result<(), StoreError> {
        self.store
            .finish_run(run_id, TerminalState::Failed { error })
            .await?;
        Ok(())
    }
}
