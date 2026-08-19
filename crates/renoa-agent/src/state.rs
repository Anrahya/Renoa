use serde::{Deserialize, Serialize};

use crate::{Message, ToolOutcomeUnknown};

/// Portable active agent state; the host owns authoritative session history.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentState {
    pub(crate) messages: Vec<Message>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(crate) unresolved_tool_outcomes: Vec<ToolOutcomeUnknown>,
}

impl AgentState {
    /// Creates an active transcript that the host asserts has no unresolved
    /// tool outcomes.
    #[must_use]
    pub fn from_messages(messages: Vec<Message>) -> Self {
        Self {
            messages,
            unresolved_tool_outcomes: Vec::new(),
        }
    }

    #[must_use]
    pub fn messages(&self) -> &[Message] {
        &self.messages
    }

    /// Tool calls that may have completed but have no authoritative result.
    #[must_use]
    pub fn unresolved_tool_outcomes(&self) -> &[ToolOutcomeUnknown] {
        &self.unresolved_tool_outcomes
    }
}
