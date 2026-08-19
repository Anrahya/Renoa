use renoa_agent::{ContentBlock, Message};
use renoa_kernel::{Checkpoint, LoopError, NewEvent, SemanticEvent};
use serde::{Deserialize, Serialize};

use crate::{
    configuration::CHECKPOINT_SCHEMA_VERSION,
    context::{ContextInput, ContextOrigin},
};

/// Versioned semantic-event kind carrying one provider-neutral message.
pub const MESSAGE_EVENT_KIND: &str = "renoa.agent.message.v1";
const MESSAGE_EVENT_PREFIX: &str = "renoa.agent.message.";

/// Command content consumed by the model/tool loop.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AgentCommand {
    content: Vec<ContentBlock>,
}

impl AgentCommand {
    #[must_use]
    pub fn new(content: Vec<ContentBlock>) -> Self {
        Self { content }
    }

    #[must_use]
    pub fn text(text: impl Into<String>) -> Self {
        Self::new(vec![ContentBlock::text(text)])
    }

    #[must_use]
    pub fn content(&self) -> &[ContentBlock] {
        &self.content
    }

    pub(crate) fn into_message(self) -> Message {
        Message::User {
            content: self.content,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "phase", rename_all = "snake_case", deny_unknown_fields)]
pub(crate) enum LoopPhase {
    NeedModel {
        model_turns: u32,
    },
    AwaitingModel {
        model_turns: u32,
    },
    NeedTool {
        model_turns: u32,
        calls: Vec<renoa_agent::ToolCall>,
        next_index: u32,
    },
    AwaitingTool {
        model_turns: u32,
        calls: Vec<renoa_agent::ToolCall>,
        next_index: u32,
    },
    Terminal,
}

pub(crate) fn checkpoint(phase: LoopPhase) -> Result<Checkpoint, LoopError> {
    serde_json::to_value(phase)
        .map(|state| Checkpoint::new(CHECKPOINT_SCHEMA_VERSION, state))
        .map_err(|error| LoopError::new(format!("agent checkpoint encoding failed: {error}")))
}

pub(crate) fn decode_checkpoint(checkpoint: &Checkpoint) -> Result<LoopPhase, LoopError> {
    serde_json::from_value(checkpoint.state().clone())
        .map_err(|error| LoopError::new(format!("agent checkpoint is invalid: {error}")))
}

pub(crate) fn message_event(message: Message) -> Result<NewEvent, LoopError> {
    serde_json::to_value(message)
        .map(|payload| NewEvent::new(MESSAGE_EVENT_KIND, payload))
        .map_err(|error| LoopError::new(format!("message event encoding failed: {error}")))
}

pub(crate) fn message_events(
    messages: impl IntoIterator<Item = Message>,
) -> Result<Vec<NewEvent>, LoopError> {
    messages.into_iter().map(message_event).collect()
}

pub(crate) fn context_input(
    active_operation_id: renoa_kernel::OperationId,
    events: &[SemanticEvent],
) -> Result<ContextInput, LoopError> {
    let mut entries = Vec::new();
    for event in events {
        if event.kind == MESSAGE_EVENT_KIND {
            let message = serde_json::from_value(event.payload.clone()).map_err(|error| {
                LoopError::new(format!(
                    "message event {} cannot be decoded: {error}",
                    event.event_id
                ))
            })?;
            entries.push((
                ContextOrigin::new(event.operation_id, event.sequence),
                message,
            ));
        } else if event.kind.starts_with(MESSAGE_EVENT_PREFIX) {
            return Err(LoopError::new(format!(
                "message event kind `{}` is unsupported",
                event.kind
            )));
        }
    }
    Ok(ContextInput::new(active_operation_id, entries))
}
