use std::fmt;

use renoa_agent::{ContentBlock, Message, StopReason, TokenUsage, ToolSpec};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

pub(crate) const STORED_STATE_VERSION: u32 = 2;

macro_rules! harness_id {
    ($name:ident) => {
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
        #[serde(transparent)]
        pub struct $name(Uuid);

        #[allow(
            clippy::new_without_default,
            reason = "identity creation should remain explicit at call sites"
        )]
        impl $name {
            #[must_use]
            pub const fn from_uuid(value: Uuid) -> Self {
                Self(value)
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                self.0.fmt(formatter)
            }
        }
    };
}

harness_id!(SessionId);
harness_id!(OperationId);
harness_id!(OutputId);
harness_id!(CancellationId);

#[allow(
    clippy::new_without_default,
    reason = "identity creation should remain explicit at call sites"
)]
impl SessionId {
    /// Creates the stable identity used to create or recover a session.
    #[must_use]
    pub fn new() -> Self {
        Self(Uuid::new_v4())
    }
}

impl OperationId {
    pub(crate) fn new() -> Self {
        Self(Uuid::new_v4())
    }
}

#[allow(
    clippy::new_without_default,
    reason = "identity creation should remain explicit at call sites"
)]
impl CancellationId {
    /// Creates the stable identity of one cancellation request.
    #[must_use]
    pub fn new() -> Self {
        Self(Uuid::new_v4())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct RequestId(Uuid);

#[allow(
    clippy::new_without_default,
    reason = "identity creation should remain explicit at call sites"
)]
impl RequestId {
    #[must_use]
    pub fn new() -> Self {
        Self(Uuid::new_v4())
    }

    #[must_use]
    pub const fn from_uuid(value: Uuid) -> Self {
        Self(value)
    }
}

impl fmt::Display for RequestId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(formatter)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OperationRequest {
    request_id: RequestId,
    content: Vec<ContentBlock>,
}

impl OperationRequest {
    #[must_use]
    pub fn new(request_id: RequestId, content: Vec<ContentBlock>) -> Self {
        Self {
            request_id,
            content,
        }
    }

    pub(crate) const fn request_id(&self) -> RequestId {
        self.request_id
    }

    pub(crate) fn into_message(self) -> Message {
        Message::User {
            content: self.content,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub struct Admission {
    pub operation_id: OperationId,
    pub position: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum OperationStatus {
    Queued,
    Running,
    OutcomeUnknown,
    Completed,
    Cancelled,
    Failed,
}

#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct OperationSnapshot {
    pub operation_id: OperationId,
    pub position: u64,
    pub status: OperationStatus,
}

#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct SessionSnapshot {
    pub messages: Vec<Message>,
    pub operations: Vec<OperationSnapshot>,
    pub outputs: Vec<OutputRecord>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct OutputRecord {
    pub output_id: OutputId,
    pub sequence: u64,
    pub operation_id: OperationId,
    pub outcome: OperationOutcome,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case")]
#[non_exhaustive]
pub enum OperationOutcome {
    Completed {
        output: String,
        stop_reason: StopReason,
        usage: Option<TokenUsage>,
    },
    Failed {
        message: String,
    },
    Cancelled {
        message: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum RunNext {
    Idle,
    Blocked {
        operation_id: OperationId,
    },
    Finished {
        operation_id: OperationId,
        outcome: OperationOutcome,
    },
}

/// Whether a pending tool effect may be repeated after its outcome is lost.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum ToolRecovery {
    SafeToReplay,
    NeverReplay,
}

impl ToolRecovery {
    pub(crate) const fn as_str(self) -> &'static str {
        match self {
            Self::SafeToReplay => "safe_to_replay",
            Self::NeverReplay => "never_replay",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct FrozenTool {
    pub(crate) spec: ToolSpec,
    pub(crate) recovery: ToolRecovery,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct FrozenRuntime {
    pub(crate) revision: String,
    pub(crate) system_prompt: String,
    pub(crate) max_model_attempts: u32,
    pub(crate) max_tool_calls_per_step: u32,
    pub(crate) tools: Vec<FrozenTool>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct OperationProgress {
    pub(crate) runtime: FrozenRuntime,
    pub(crate) model_attempts: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct ToolBatch {
    pub(crate) batch_id: Uuid,
    pub(crate) next_index: u32,
    pub(crate) call_count: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct StoredState {
    format_version: u32,
    state: StoredOperationState,
}

impl StoredState {
    pub(crate) const fn queued() -> Self {
        Self {
            format_version: STORED_STATE_VERSION,
            state: StoredOperationState::Queued,
        }
    }

    pub(crate) fn status(&self) -> OperationStatus {
        match &self.state {
            StoredOperationState::Queued => OperationStatus::Queued,
            StoredOperationState::NeedModel { .. }
            | StoredOperationState::ModelPending { .. }
            | StoredOperationState::NeedTool { .. }
            | StoredOperationState::ToolPending { .. } => OperationStatus::Running,
            StoredOperationState::ToolOutcomeUnknown { .. } => OperationStatus::OutcomeUnknown,
            StoredOperationState::Completed => OperationStatus::Completed,
            StoredOperationState::Failed {
                kind: FailureKind::Cancelled,
            } => OperationStatus::Cancelled,
            StoredOperationState::Failed { .. } => OperationStatus::Failed,
        }
    }

    pub(crate) const fn format_version(&self) -> u32 {
        self.format_version
    }

    pub(crate) const fn state(&self) -> &StoredOperationState {
        &self.state
    }

    pub(crate) const fn from_state(state: StoredOperationState) -> Self {
        Self {
            format_version: STORED_STATE_VERSION,
            state,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "phase", rename_all = "snake_case")]
pub(crate) enum StoredOperationState {
    Queued,
    NeedModel {
        progress: OperationProgress,
    },
    ModelPending {
        progress: OperationProgress,
        effect_id: Uuid,
        settlement_token: Uuid,
        assistant_entry_id: Uuid,
        output_id: Uuid,
    },
    NeedTool {
        progress: OperationProgress,
        batch: ToolBatch,
    },
    ToolPending {
        progress: OperationProgress,
        batch: ToolBatch,
        effect_id: Uuid,
        settlement_token: Uuid,
        recovery: ToolRecovery,
    },
    ToolOutcomeUnknown {
        progress: OperationProgress,
        batch: ToolBatch,
    },
    Completed,
    Failed {
        kind: FailureKind,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum FailureKind {
    General,
    AbandonedUnknownTool,
    Cancelled,
}
