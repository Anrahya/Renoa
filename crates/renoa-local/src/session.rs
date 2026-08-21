use std::{path::Path, time::Duration};

use renoa_agent::{AssistantContent, ContentBlock, Message, StopReason};
use renoa_agent_loop::{AgentCommand, MESSAGE_EVENT_KIND};
use renoa_kernel::{
    AgentId, CancellationId, Command, CommandId, DriveResult, EventCursor, Kernel, KernelError,
    OperationId, OperationOutcome, Runtime, SessionId,
};
use thiserror::Error;
use tokio_util::sync::CancellationToken;

/// One local Alpha conversation owned durably by the kernel.
pub struct LocalSession {
    kernel: Kernel,
    agent_id: AgentId,
    session_id: SessionId,
}

/// One kernel-persisted conversation message with stable surface identity.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LocalHistoryEntry {
    pub event_id: String,
    pub message: Message,
}

/// Host-visible result of one exact local coding turn.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum LocalTurnOutcome {
    Completed {
        output: String,
        stop_reason: StopReason,
    },
    Cancelled,
    Failed {
        reason: String,
    },
    WaitingForInput,
}

/// Failure to admit, drive, or project one local Host turn.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum LocalSessionError {
    #[error(transparent)]
    Kernel(#[from] KernelError),
    #[error("local agent command cannot be encoded: {0}")]
    CommandEncoding(#[source] serde_json::Error),
    #[error("admitted operation {0} is absent from its kernel session")]
    AdmissionMissing(OperationId),
    #[error(
        "session has unfinished operation {operation_id} for command {command_id}; retry that command before submitting another"
    )]
    UnfinishedOperation {
        operation_id: OperationId,
        command_id: CommandId,
    },
    #[error(
        "command {command_id} is already bound to operation {operation_id} with different content"
    )]
    CommandConflict {
        command_id: CommandId,
        operation_id: OperationId,
    },
    #[error("kernel finished operation {actual} before admitted operation {admitted}")]
    EarlierOperationFinished {
        admitted: OperationId,
        actual: OperationId,
    },
    #[error("kernel blocked on operation {actual} before admitted operation {admitted}")]
    EarlierOperationBlocked {
        admitted: OperationId,
        actual: OperationId,
    },
    #[error("admitted operation {0} had no durable outcome")]
    Idle(OperationId),
    #[error("completed operation {0} has no durable assistant message")]
    MessageMissing(OperationId),
    #[error("durable message for operation {operation_id} is invalid: {source}")]
    MessageInvalid {
        operation_id: OperationId,
        #[source]
        source: serde_json::Error,
    },
    #[error("completed operation {0} ended on a non-assistant message")]
    NonAssistantMessage(OperationId),
    #[error("the kernel returned an unsupported drive result")]
    UnsupportedDriveResult,
    #[error("the kernel returned an unsupported operation outcome")]
    UnsupportedOperationOutcome,
}

impl LocalSession {
    /// Creates one exact Agent and Session in a newly opened kernel database.
    ///
    /// Repeating the same identities is idempotent when their binding matches.
    ///
    /// # Errors
    ///
    /// Returns a kernel ownership, storage, identity, or binding failure.
    pub fn create(
        database: impl AsRef<Path>,
        agent_id: AgentId,
        session_id: SessionId,
    ) -> Result<Self, LocalSessionError> {
        let kernel = Kernel::open(database)?;
        kernel.create_agent(agent_id)?;
        kernel.create_session(session_id, agent_id)?;
        Ok(Self {
            kernel,
            agent_id,
            session_id,
        })
    }

    /// Opens one existing local Session and recovers its Agent binding.
    ///
    /// # Errors
    ///
    /// Returns a kernel ownership, storage, compatibility, or not-found failure.
    pub fn load(
        database: impl AsRef<Path>,
        session_id: SessionId,
    ) -> Result<Self, LocalSessionError> {
        let kernel = Kernel::open(database)?;
        let snapshot = kernel.inspect(session_id)?;
        Ok(Self {
            kernel,
            agent_id: snapshot.agent_id,
            session_id,
        })
    }

    #[must_use]
    pub const fn agent_id(&self) -> AgentId {
        self.agent_id
    }

    #[must_use]
    pub const fn session_id(&self) -> SessionId {
        self.session_id
    }

    /// Projects the complete gapless durable conversation for surface replay.
    ///
    /// # Errors
    ///
    /// Returns kernel corruption or an invalid persisted message payload.
    pub fn history(&self) -> Result<Vec<LocalHistoryEntry>, LocalSessionError> {
        self.kernel
            .events_after(self.session_id, EventCursor::START)?
            .events
            .into_iter()
            .filter(|event| event.kind == MESSAGE_EVENT_KIND)
            .map(|event| {
                let message =
                    serde_json::from_value::<Message>(event.payload).map_err(|source| {
                        LocalSessionError::MessageInvalid {
                            operation_id: event.operation_id,
                            source,
                        }
                    })?;
                Ok(LocalHistoryEntry {
                    event_id: event.event_id.to_string(),
                    message,
                })
            })
            .collect()
    }

    /// Returns a settled durable result for this exact command without resolving a runtime.
    ///
    /// This read-only path lets a Host recover a lost successful reply even if
    /// the current provider or workspace instructions are unavailable. An
    /// absent or unfinished command returns `None`; changed content under an
    /// existing identity is still rejected.
    ///
    /// # Errors
    ///
    /// Returns command encoding, identity conflict, storage, or projection failures.
    pub fn replay_settled_turn(
        &self,
        command_id: CommandId,
        content: &[ContentBlock],
    ) -> Result<Option<LocalTurnOutcome>, LocalSessionError> {
        let encoded = encode_command(content.to_vec())?;
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

    /// Admits and drives one caller-identified Alpha command to a Host boundary.
    ///
    /// Exact redelivery returns the existing durable result. Reusing
    /// `command_id` with different content fails before model or tool work.
    /// Cancellation is durably requested and waits for active effect cleanup.
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
        if cancellation.is_cancelled() {
            return Ok(LocalTurnOutcome::Cancelled);
        }
        let command = encode_command(content)?;
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
        let event = page
            .events
            .iter()
            .rev()
            .find(|event| event.operation_id == operation_id && event.kind == MESSAGE_EVENT_KIND)
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

fn encode_command(content: Vec<ContentBlock>) -> Result<serde_json::Value, LocalSessionError> {
    serde_json::to_value(AgentCommand::new(content)).map_err(LocalSessionError::CommandEncoding)
}
