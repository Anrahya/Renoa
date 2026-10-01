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
        /// Where the surface received the text, such as the conversation it
        /// was written in, described by that surface for the agent. The executor decides whether
        /// to show it; RCP only carries it with the command.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        context: Option<String>,
        /// Who wrote the text, as the submitting surface knows them.
        #[serde(default, skip_serializing_if = "Author::is_principal")]
        author: Author,
    },
}

/// Who wrote a command's text. The coordinator authenticates only the
/// submitting surface's principal; a surface shared with other people marks
/// their text as a guest's, and the executor decides what a guest may do.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Author {
    /// The authenticated principal. Omitted on the wire, and never read from
    /// it: the only author a surface can name is a guest.
    #[default]
    #[serde(skip_deserializing)]
    Principal,
    /// Someone else in a conversation the surface shares with the principal.
    Guest,
}

impl Author {
    #[must_use]
    pub const fn is_principal(&self) -> bool {
        matches!(self, Self::Principal)
    }
}

impl CommandInput {
    /// The principal's own text, with no surface context.
    #[must_use]
    pub fn from_text(text: impl Into<String>) -> Self {
        Self::Text {
            text: text.into(),
            context: None,
            author: Author::Principal,
        }
    }

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

    #[must_use]
    pub const fn author(&self) -> Author {
        match self {
            Self::Text { author, .. } => *author,
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
