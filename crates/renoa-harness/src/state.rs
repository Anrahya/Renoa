use std::fmt;

use renoa_agent::{ContentBlock, Message, StopReason, TokenUsage};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

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
    Completed,
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
}

#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum RunNext {
    Idle,
    Finished {
        operation_id: OperationId,
        outcome: OperationOutcome,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct StoredState {
    format_version: u32,
    state: StoredOperationState,
}

impl StoredState {
    pub(crate) const fn queued() -> Self {
        Self {
            format_version: 1,
            state: StoredOperationState::Queued,
        }
    }

    pub(crate) fn status(&self) -> OperationStatus {
        match &self.state {
            StoredOperationState::Queued => OperationStatus::Queued,
            StoredOperationState::NeedModel { .. } | StoredOperationState::ModelPending { .. } => {
                OperationStatus::Running
            }
            StoredOperationState::Completed => OperationStatus::Completed,
            StoredOperationState::Failed => OperationStatus::Failed,
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
            format_version: 1,
            state,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "phase", rename_all = "snake_case")]
pub(crate) enum StoredOperationState {
    Queued,
    NeedModel {
        runtime_revision: String,
        system_prompt: String,
        max_model_attempts: u32,
        attempt_count: u32,
    },
    ModelPending {
        runtime_revision: String,
        max_model_attempts: u32,
        attempt_count: u32,
        effect_id: Uuid,
        settlement_token: Uuid,
        assistant_entry_id: Uuid,
        output_id: Uuid,
    },
    Completed,
    Failed,
}
