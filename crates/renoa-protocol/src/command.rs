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
    Text {
        text: String,
        /// Where the surface received the text, such as a Discord channel,
        /// written by that surface for the agent. The executor decides whether
        /// to show it; RCP only carries it with the command.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        context: Option<String>,
    },
}

impl CommandInput {
    #[must_use]
    pub fn text(&self) -> &str {
        match self {
            Self::Text { text, .. } => text,
        }
    }

    #[must_use]
    pub fn context(&self) -> Option<&str> {
        match self {
            Self::Text { context, .. } => context.as_deref(),
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
