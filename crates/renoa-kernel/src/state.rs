use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::{
    AgentId, CommandId, EffectId, EffectOutcome, EffectRecovery, OperationId, RuntimeManifest,
};

/// One exact caller-identified input admitted to a session.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Command {
    command_id: CommandId,
    content: Value,
}

impl Command {
    #[must_use]
    pub const fn new(command_id: CommandId, content: Value) -> Self {
        Self {
            command_id,
            content,
        }
    }

    #[must_use]
    pub const fn command_id(&self) -> CommandId {
        self.command_id
    }

    pub(crate) fn into_command_id(self) -> CommandId {
        self.command_id
    }

    #[must_use]
    pub const fn content(&self) -> &Value {
        &self.content
    }
}

/// The stable result of durable command admission.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub struct Admission {
    pub operation_id: OperationId,
    pub position: u64,
}

/// The externally meaningful lifecycle of an admitted operation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum OperationStatus {
    Queued,
    Running,
    OutcomeUnknown,
    Waiting,
    Completed,
    Failed,
}

/// The terminal reason an operation released its session.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "outcome", rename_all = "snake_case")]
#[non_exhaustive]
pub enum OperationOutcome {
    WaitingForInput,
    Completed,
    Failed { reason: String },
}

/// The durable lifecycle of one external effect.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum EffectStatus {
    IntentCommitted,
    DispatchStarted,
    Settled,
    OutcomeUnknown,
}

/// A read-only view of one exact external effect.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct EffectSnapshot {
    pub effect_id: EffectId,
    pub position: u64,
    pub binding: String,
    pub binding_revision: String,
    pub recovery: EffectRecovery,
    pub request: Value,
    pub status: EffectStatus,
    pub dispatch_count: u64,
    pub outcome: Option<EffectOutcome>,
}

/// A read-only view of one operation.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct OperationSnapshot {
    pub operation_id: OperationId,
    pub command_id: CommandId,
    pub command: Command,
    pub position: u64,
    pub status: OperationStatus,
    pub manifest: Option<RuntimeManifest>,
    pub checkpoint: Option<crate::Checkpoint>,
    pub outcome: Option<OperationOutcome>,
    pub effects: Vec<EffectSnapshot>,
}

/// A transactionally consistent read of one isolated session.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct SessionSnapshot {
    pub agent_id: AgentId,
    pub operations: Vec<OperationSnapshot>,
}

/// The result of driving at most one operation to a host-visible boundary.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum DriveResult {
    Idle,
    Blocked {
        operation_id: OperationId,
    },
    Finished {
        operation_id: OperationId,
        outcome: OperationOutcome,
    },
}
