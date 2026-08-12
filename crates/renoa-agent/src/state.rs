use serde::{Deserialize, Serialize};

use crate::Message;

/// Portable active transcript; the host owns authoritative session history.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentState {
    pub(crate) messages: Vec<Message>,
}

impl AgentState {
    /// Creates an active transcript selected by the host.
    #[must_use]
    pub fn from_messages(messages: Vec<Message>) -> Self {
        Self { messages }
    }

    #[must_use]
    pub fn messages(&self) -> &[Message] {
        &self.messages
    }
}
