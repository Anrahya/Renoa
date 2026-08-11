use std::sync::Arc;

use renoa_core::{
    CapabilityHost, CapabilityOutcome, CapabilityRequest, CommandEnvelope, Message, ModelDriver,
    ModelError, ModelRequest, ResolvedAgent, RunAdmission, RunEventKind, RunId, RunStore,
    StoreError, TerminalState,
};
use thiserror::Error;
use tokio_util::sync::CancellationToken;

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
        self.run_in_context(command, &agent, &mut messages, cancellation)
            .await
    }

    pub(crate) async fn run_in_context(
        &self,
        command: CommandEnvelope,
        agent: &ResolvedAgent,
        messages: &mut Vec<Message>,
        cancellation: CancellationToken,
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
        messages.push(Message::User {
            text: command.input.text().to_owned(),
        });

        for round in 0..self.config.max_model_rounds {
            if cancellation.is_cancelled() {
                return self.terminal_error(run_id, EngineError::Cancelled).await;
            }

            let response = match self
                .model_step(
                    run_id,
                    round,
                    &agent.instructions,
                    messages,
                    &capabilities,
                    cancellation.child_token(),
                )
                .await
            {
                Ok(response) => response,
                Err(error) => {
                    return self.terminal_error(run_id, error).await;
                }
            };

            messages.push(Message::Assistant {
                text: response.text.clone(),
                capability_calls: response.capability_calls.clone(),
            });

            if response.capability_calls.is_empty() {
                return self.complete_run(run_id, response.text, round + 1).await;
            }

            let outcomes = match self
                .capability_batch(
                    run_id,
                    &command,
                    &response,
                    &capabilities,
                    cancellation.child_token(),
                )
                .await
            {
                Ok(outcomes) => outcomes,
                Err(error) => return self.terminal_error(run_id, error).await,
            };

            for (request, outcome) in outcomes {
                messages.push(Message::Capability {
                    call_id: request.call.call_id,
                    name: request.call.name,
                    outcome,
                });
            }

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

    async fn model_step(
        &self,
        run_id: RunId,
        round: u32,
        instructions: &str,
        messages: &[Message],
        capabilities: &[renoa_core::CapabilitySpec],
        cancellation: CancellationToken,
    ) -> Result<renoa_core::ModelResponse, EngineError> {
        let mut model_messages = Vec::with_capacity(messages.len() + 1);
        model_messages.push(Message::System {
            text: instructions.to_owned(),
        });
        model_messages.extend_from_slice(messages);
        let request = ModelRequest {
            run_id,
            round,
            messages: model_messages,
            capabilities: capabilities.to_vec(),
        };
        self.store
            .append_events(run_id, vec![RunEventKind::ModelRequested { round }])
            .await?;
        let response = tokio::select! {
            () = cancellation.cancelled() => return Err(EngineError::Cancelled),
            result = self.model.generate(request, cancellation.child_token()) => result?,
        };
        self.store
            .append_events(
                run_id,
                vec![RunEventKind::ModelResponded {
                    round,
                    response: response.clone(),
                }],
            )
            .await?;
        Ok(response)
    }

    async fn capability_batch(
        &self,
        run_id: RunId,
        command: &CommandEnvelope,
        response: &renoa_core::ModelResponse,
        capabilities: &[renoa_core::CapabilitySpec],
        cancellation: CancellationToken,
    ) -> Result<Vec<(CapabilityRequest, CapabilityOutcome)>, EngineError> {
        if response.capability_calls.len() > self.config.max_capability_calls_per_response {
            return Err(EngineError::CapabilityBatchTooLarge {
                actual: response.capability_calls.len(),
                limit: self.config.max_capability_calls_per_response,
            });
        }
        let requests = response
            .capability_calls
            .iter()
            .enumerate()
            .map(|(ordinal, call)| {
                let ordinal =
                    u32::try_from(ordinal).map_err(|_| EngineError::CapabilityBatchTooLarge {
                        actual: response.capability_calls.len(),
                        limit: self.config.max_capability_calls_per_response,
                    })?;
                Ok(CapabilityRequest {
                    run_id,
                    target: command.target.clone(),
                    ordinal,
                    call: call.clone(),
                })
            })
            .collect::<Result<Vec<_>, EngineError>>()?;

        self.store
            .append_events(
                run_id,
                requests
                    .iter()
                    .map(|request| RunEventKind::CapabilityRequested {
                        ordinal: request.ordinal,
                        call: request.call.clone(),
                    })
                    .collect(),
            )
            .await?;
        let outcomes = if response.truncated {
            let mut outcomes = Vec::with_capacity(requests.len());
            for request in requests {
                let outcome = CapabilityOutcome::error(
                    "capability call was not executed because the model response was truncated",
                );
                self.record_capability_completion(run_id, &request, &outcome)
                    .await?;
                outcomes.push((request, outcome));
            }
            outcomes
        } else {
            self.execute_capabilities(run_id, requests, capabilities, cancellation)
                .await?
        };
        Ok(outcomes)
    }

    async fn execute_capabilities(
        &self,
        run_id: RunId,
        requests: Vec<CapabilityRequest>,
        capabilities: &[renoa_core::CapabilitySpec],
        cancellation: CancellationToken,
    ) -> Result<Vec<(CapabilityRequest, CapabilityOutcome)>, EngineError> {
        let mut tasks = tokio::task::JoinSet::new();
        let mut outcomes = Vec::with_capacity(requests.len());
        for request in requests {
            if !capabilities
                .iter()
                .any(|capability| capability.name == request.call.name)
            {
                let message = format!("capability `{}` is not granted", request.call.name);
                let outcome = CapabilityOutcome::error(message);
                self.record_capability_completion(run_id, &request, &outcome)
                    .await?;
                outcomes.push((request, outcome));
                continue;
            }
            let host = Arc::clone(&self.capabilities);
            let child_cancellation = cancellation.child_token();
            tasks.spawn(async move {
                let outcome = tokio::select! {
                    () = child_cancellation.cancelled() => {
                        CapabilityOutcome::error("capability execution was cancelled")
                    }
                    outcome = host.execute(request.clone(), child_cancellation.child_token()) => outcome,
                };
                (request, outcome)
            });
            let Some(result) = tasks.join_next().await else {
                return Err(EngineError::CapabilityTask(
                    "capability task disappeared before completion".to_owned(),
                ));
            };
            let (request, outcome) =
                result.map_err(|error| EngineError::CapabilityTask(error.to_string()))?;
            self.record_capability_completion(run_id, &request, &outcome)
                .await?;
            outcomes.push((request, outcome));
        }
        Ok(outcomes)
    }

    async fn record_capability_completion(
        &self,
        run_id: RunId,
        request: &CapabilityRequest,
        outcome: &CapabilityOutcome,
    ) -> Result<(), StoreError> {
        self.store
            .append_events(
                run_id,
                vec![RunEventKind::CapabilityCompleted {
                    ordinal: request.ordinal,
                    call_id: request.call.call_id.clone(),
                    outcome: outcome.clone(),
                }],
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
