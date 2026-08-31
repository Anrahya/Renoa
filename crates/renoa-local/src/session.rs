use std::path::Path;

use renoa_agent::{Message, StopReason};
use renoa_agent_loop::MESSAGE_EVENT_KIND;
use renoa_kernel::{AgentId, CommandId, EventCursor, Kernel, KernelError, OperationId, SessionId};
use thiserror::Error;

use crate::TurnObservationError;

mod execution;
#[cfg(test)]
mod timing_tests;

/// One local Agent conversation owned durably by the kernel.
pub struct LocalSession {
    kernel: Kernel,
    agent_id: AgentId,
    session_id: SessionId,
}

/// One kernel-persisted conversation message with stable surface identity.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LocalHistoryEntry {
    pub event_id: String,
    pub command_id: CommandId,
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
    Compacted {
        estimated_input_tokens: u64,
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
    #[error("durable command for operation {operation_id} is invalid: {source}")]
    CommandInvalid {
        operation_id: OperationId,
        #[source]
        source: serde_json::Error,
    },
    #[error(transparent)]
    TurnObservation(#[from] TurnObservationError),
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
    #[error("durable compaction result for operation {operation_id} is invalid: {source}")]
    CompactionResultInvalid {
        operation_id: OperationId,
        #[source]
        source: serde_json::Error,
    },
    #[error("completed operation {0} contains more than one durable compaction result")]
    DuplicateCompactionResult(OperationId),
    #[error("completed operation {0} contains both conversation and compaction results")]
    MixedCompletedResult(OperationId),
    #[error("durable token usage for operation {0} overflowed u64")]
    TokenUsageOverflow(OperationId),
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
                    command_id: event.command_id,
                    message,
                })
            })
            .collect()
    }
}
