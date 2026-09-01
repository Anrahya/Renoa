use serde::{Deserialize, Serialize};

use crate::{CommandId, PrincipalId};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct SurfaceRef(String);

impl SurfaceRef {
    #[must_use]
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct TargetRef(String);

impl TargetRef {
    #[must_use]
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum CommandInput {
    Text { text: String },
}

impl CommandInput {
    #[must_use]
    pub fn text(&self) -> &str {
        match self {
            Self::Text { text } => text,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CommandEnvelope {
    pub command_id: CommandId,
    pub principal_id: PrincipalId,
    pub surface: SurfaceRef,
    pub target: TargetRef,
    pub input: CommandInput,
}
