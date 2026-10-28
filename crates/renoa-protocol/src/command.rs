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

/// One agent a node can execute for, as the node describes it when it
/// advertises: the opaque target, and the name its owner sees and other
/// agents address it by.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TargetCard {
    pub target: TargetRef,
    pub name: String,
}

impl TargetCard {
    /// The most bytes a target or a name may have.
    pub const MAX_LABEL_BYTES: usize = 256;

    /// Whether a target or name is acceptable on the wire: 1 to
    /// [`Self::MAX_LABEL_BYTES`] bytes with no control character.
    #[must_use]
    pub fn label_is_valid(label: &str) -> bool {
        !label.is_empty()
            && label.len() <= Self::MAX_LABEL_BYTES
            && !label.chars().any(char::is_control)
    }

    /// A card for `target` named `name`, with the name made acceptable: control
    /// characters become spaces and an overlong name is cut at a character
    /// boundary. A name left blank is the target itself.
    #[must_use]
    pub fn new(target: TargetRef, name: &str) -> Self {
        let mut name = name
            .chars()
            .map(|c| if c.is_control() { ' ' } else { c })
            .collect::<String>();
        let mut end = name.len().min(Self::MAX_LABEL_BYTES);
        while !name.is_char_boundary(end) {
            end -= 1;
        }
        name.truncate(end);
        let name = name.trim();
        let name = if name.is_empty() {
            target.as_str().to_owned()
        } else {
            name.to_owned()
        };
        Self { target, name }
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

/// Why a command exists when another agent caused it. Only the coordinator
/// writes it, from facts it holds: no surface can submit one.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentOrigin {
    /// The sending agent's command that caused this one. For a reply or a
    /// failure it is the request being answered.
    pub from_command: CommandId,
    /// The sending agent's target.
    pub from: TargetRef,
    /// The sending agent's name when the coordinator admitted the command.
    pub name: String,
    /// Agent-to-agent steps since a command no agent caused.
    pub depth: u32,
    pub kind: AgentMessageKind,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentMessageKind {
    /// An agent asked this one something. Its execution is answered to the
    /// asker's task.
    Request,
    /// The answer to a request: the last assistant message of its execution.
    Reply,
    /// The request's execution failed or was cancelled; the text says why.
    Failure,
}

impl AgentMessageKind {
    /// Whether the execution of a command of this kind is answered to its
    /// sender. A reply and a failure start nothing by themselves.
    #[must_use]
    pub const fn expects_reply(self) -> bool {
        matches!(self, Self::Request)
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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub origin: Option<AgentOrigin>,
}
