use std::time::Duration;

use renoa_agent::{AssistantContent, ContentBlock, Message, TokenUsage};
use renoa_agent_loop::{
    AgentCommand, COMPACTION_RESULT_EVENT_KIND, CompactionResult, MESSAGE_EVENT_KIND,
};
use renoa_kernel::{
    CancellationId, Command, CommandId, DriveResult, EventCursor, KernelError, OperationId,
    OperationOutcome, Runtime,
};
use tokio_util::sync::CancellationToken;

use super::{LocalSession, LocalSessionError, LocalTurnOutcome};

impl LocalSession {
    /// Returns a settled durable prompt result without resolving a runtime.
    ///
    /// # Errors
    ///
    /// Returns command encoding, identity conflict, storage, or projection failures.
    pub fn replay_settled_turn(
        &self,
        command_id: CommandId,
        content: &[ContentBlock],
    ) -> Result<Option<LocalTurnOutcome>, LocalSessionError> {
        self.replay_settled_command(command_id, AgentCommand::new(content.to_vec()))
    }

    /// Returns a settled durable explicit-compaction result without resolving a runtime.
    ///
    /// # Errors
    ///
    /// Returns command encoding, identity conflict, storage, or projection failures.
    pub fn replay_settled_compaction(
        &self,
        command_id: CommandId,
    ) -> Result<Option<LocalTurnOutcome>, LocalSessionError> {
        self.replay_settled_command(command_id, AgentCommand::compact())
    }

    fn replay_settled_command(
        &self,
        command_id: CommandId,
        command: AgentCommand,
    ) -> Result<Option<LocalTurnOutcome>, LocalSessionError> {
        let encoded = encode_command(command)?;
        let snapshot = self.kernel.inspect(self.session_id)?;
        let Some(operation) = snapshot
            .operations
            .iter()
            .find(|operation| operation.command_id == command_id)
        else {
            return Ok(None);
        };
        if operation.command.content() != &encoded {
            return Err(LocalSessionError::CommandConflict {
                command_id,
                operation_id: operation.operation_id,
            });
        }
        operation
            .outcome
            .clone()
            .map(|outcome| self.project_outcome(operation.operation_id, outcome))
            .transpose()
    }

    /// Admits and drives one caller-identified prompt to a Host boundary.
    ///
    /// # Errors
    ///
    /// Returns typed admission, runtime, recovery, ordering, or projection failures.
    pub async fn execute_turn(
        &self,
        command_id: CommandId,
        content: Vec<ContentBlock>,
        runtime: &Runtime,
        cancellation: CancellationToken,
    ) -> Result<LocalTurnOutcome, LocalSessionError> {
        self.execute_command(
            command_id,
            AgentCommand::new(content),
            runtime,
            cancellation,
        )
        .await
    }

    /// Admits and drives one caller-identified explicit compaction operation.
    ///
    /// # Errors
    ///
    /// Returns typed admission, runtime, recovery, ordering, or projection failures.
    pub async fn execute_compaction(
        &self,
        command_id: CommandId,
        runtime: &Runtime,
        cancellation: CancellationToken,
    ) -> Result<LocalTurnOutcome, LocalSessionError> {
        self.execute_command(command_id, AgentCommand::compact(), runtime, cancellation)
            .await
    }

    async fn execute_command(
        &self,
        command_id: CommandId,
        command: AgentCommand,
        runtime: &Runtime,
        cancellation: CancellationToken,
    ) -> Result<LocalTurnOutcome, LocalSessionError> {
        if cancellation.is_cancelled() {
            return Ok(LocalTurnOutcome::Cancelled);
        }
        let command = encode_command(command)?;
        let admission = match self
            .kernel
            .submit_exclusive(self.session_id, Command::new(command_id, command))
        {
            Ok(admission) => admission,
            Err(KernelError::UnfinishedOperation {
                operation_id,
                command_id,
            }) => {
                return Err(LocalSessionError::UnfinishedOperation {
                    operation_id,
                    command_id,
                });
            }
            Err(KernelError::CommandConflict {
                command_id,
                operation_id,
            }) => {
                return Err(LocalSessionError::CommandConflict {
                    command_id,
                    operation_id,
                });
            }
            Err(error) => return Err(error.into()),
        };
        if let Some(outcome) = self.admitted_outcome(admission.operation_id)? {
            return self.project_outcome(admission.operation_id, outcome);
        }

        let execution = self.kernel.drive(self.session_id, runtime);
        tokio::pin!(execution);
        let run = tokio::select! {
            biased;
            () = cancellation.cancelled() => {
                let cancellation_id = CancellationId::new();
                loop {
                    match self.kernel.request_cancellation(
                        self.session_id,
                        admission.operation_id,
                        cancellation_id,
                    ) {
                        Ok(()) => break execution.await?,
                        Err(KernelError::OperationNotCancellable(_)) => {
                            if let Ok(result) =
                                tokio::time::timeout(Duration::from_millis(2), &mut execution).await
                            {
                                break result?;
                            }
                        }
                        Err(error) => return Err(error.into()),
                    }
                }
            },
            result = &mut execution => result?,
        };
        match run {
            DriveResult::Finished {
                operation_id,
                outcome,
            } if operation_id == admission.operation_id => {
                self.project_outcome(operation_id, outcome)
            }
            DriveResult::Finished { operation_id, .. } => {
                Err(LocalSessionError::EarlierOperationFinished {
                    admitted: admission.operation_id,
                    actual: operation_id,
                })
            }
            DriveResult::Blocked { operation_id } if operation_id == admission.operation_id => {
                let outcome =
                    self.kernel
                        .abandon_unknown_effect(self.session_id, operation_id, runtime)?;
                self.project_outcome(operation_id, outcome)
            }
            DriveResult::Blocked { operation_id } => {
                Err(LocalSessionError::EarlierOperationBlocked {
                    admitted: admission.operation_id,
                    actual: operation_id,
                })
            }
            DriveResult::Idle => Err(LocalSessionError::Idle(admission.operation_id)),
            _ => Err(LocalSessionError::UnsupportedDriveResult),
        }
    }

    /// Returns the newest durable provider usage or post-compaction estimate.
    ///
    /// # Errors
    ///
    /// Returns storage, payload decoding, or checked token-arithmetic failures.
    pub fn latest_context_tokens(&self) -> Result<Option<u64>, LocalSessionError> {
        let mut latest = None;
        for event in self
            .kernel
            .events_after(self.session_id, EventCursor::START)?
            .events
        {
            if event.kind == MESSAGE_EVENT_KIND {
                let message =
                    serde_json::from_value::<Message>(event.payload).map_err(|source| {
                        LocalSessionError::MessageInvalid {
                            operation_id: event.operation_id,
                            source,
                        }
                    })?;
                if let Message::Assistant { usage, .. } = message {
                    latest = match usage {
                        Some(usage) => Some(
                            context_tokens(usage)
                                .ok_or(LocalSessionError::TokenUsageOverflow(event.operation_id))?,
                        ),
                        None => None,
                    };
                }
            } else if event.kind == COMPACTION_RESULT_EVENT_KIND {
                let result = decode_compaction_result(event.operation_id, event.payload)?;
                latest = Some(result.estimated_input_tokens());
            }
        }
        Ok(latest)
    }

    fn admitted_outcome(
        &self,
        operation_id: OperationId,
    ) -> Result<Option<OperationOutcome>, LocalSessionError> {
        let snapshot = self.kernel.inspect(self.session_id)?;
        let admitted = snapshot
            .operations
            .iter()
            .find(|operation| operation.operation_id == operation_id)
            .ok_or(LocalSessionError::AdmissionMissing(operation_id))?;
        if admitted.outcome.is_none()
            && let Some(operation) = snapshot
                .operations
                .iter()
                .find(|operation| operation.outcome.is_none())
            && operation.operation_id != operation_id
        {
            return Err(LocalSessionError::UnfinishedOperation {
                operation_id: operation.operation_id,
                command_id: operation.command_id,
            });
        }
        Ok(admitted.outcome.clone())
    }

    fn project_outcome(
        &self,
        operation_id: OperationId,
        outcome: OperationOutcome,
    ) -> Result<LocalTurnOutcome, LocalSessionError> {
        match outcome {
            OperationOutcome::Completed => self.completed_outcome(operation_id),
            OperationOutcome::Cancelled => Ok(LocalTurnOutcome::Cancelled),
            OperationOutcome::Failed { reason } => Ok(LocalTurnOutcome::Failed { reason }),
            OperationOutcome::WaitingForInput => Ok(LocalTurnOutcome::WaitingForInput),
            _ => Err(LocalSessionError::UnsupportedOperationOutcome),
        }
    }

    fn completed_outcome(
        &self,
        operation_id: OperationId,
    ) -> Result<LocalTurnOutcome, LocalSessionError> {
        let page = self
            .kernel
            .events_after(self.session_id, EventCursor::START)?;
        let operation_events = page
            .events
            .iter()
            .filter(|event| event.operation_id == operation_id)
            .collect::<Vec<_>>();
        let compaction_results = operation_events
            .iter()
            .filter(|event| event.kind == COMPACTION_RESULT_EVENT_KIND)
            .collect::<Vec<_>>();
        if compaction_results.len() > 1 {
            return Err(LocalSessionError::DuplicateCompactionResult(operation_id));
        }
        if let Some(event) = compaction_results.first() {
            if operation_events
                .iter()
                .any(|event| event.kind == MESSAGE_EVENT_KIND)
            {
                return Err(LocalSessionError::MixedCompletedResult(operation_id));
            }
            let result = decode_compaction_result(operation_id, event.payload.clone())?;
            return Ok(LocalTurnOutcome::Compacted {
                estimated_input_tokens: result.estimated_input_tokens(),
            });
        }
        let event = operation_events
            .iter()
            .rev()
            .find(|event| event.kind == MESSAGE_EVENT_KIND)
            .ok_or(LocalSessionError::MessageMissing(operation_id))?;
        let message =
            serde_json::from_value::<Message>(event.payload.clone()).map_err(|source| {
                LocalSessionError::MessageInvalid {
                    operation_id,
                    source,
                }
            })?;
        let Message::Assistant {
            content,
            stop_reason,
            ..
        } = message
        else {
            return Err(LocalSessionError::NonAssistantMessage(operation_id));
        };
        let output = content
            .into_iter()
            .filter_map(|block| match block {
                AssistantContent::Text { text, .. } => Some(text),
                AssistantContent::Reasoning { .. } | AssistantContent::ToolCall { .. } => None,
            })
            .collect();
        Ok(LocalTurnOutcome::Completed {
            output,
            stop_reason,
        })
    }
}

fn encode_command(command: AgentCommand) -> Result<serde_json::Value, LocalSessionError> {
    serde_json::to_value(command).map_err(LocalSessionError::CommandEncoding)
}

fn decode_compaction_result(
    operation_id: OperationId,
    payload: serde_json::Value,
) -> Result<CompactionResult, LocalSessionError> {
    serde_json::from_value(payload).map_err(|source| LocalSessionError::CompactionResultInvalid {
        operation_id,
        source,
    })
}

fn context_tokens(usage: TokenUsage) -> Option<u64> {
    usage
        .input
        .checked_add(usage.cache_read)?
        .checked_add(usage.cache_write)?
        .checked_add(usage.output)
}
