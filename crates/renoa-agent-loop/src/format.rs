use std::{collections::HashSet, num::NonZeroU32};

use renoa_agent::{ContentBlock, Message, ModelResponse, ToolSpec};
use renoa_kernel::{Checkpoint, LoopError, NewEvent, SemanticEvent};
use serde::{Deserialize, Serialize};

use crate::{
    CompactionPlan,
    configuration::CHECKPOINT_SCHEMA_VERSION,
    context::{ActivatedCheckpoint, ContextInput, ContextOrigin},
};

#[cfg(test)]
mod tests;

/// Versioned semantic-event kind carrying one provider-neutral message.
pub const MESSAGE_EVENT_KIND: &str = "renoa.agent.message.v1";
const MESSAGE_EVENT_PREFIX: &str = "renoa.agent.message.";
/// Versioned semantic-event kind carrying one activated portable summary.
pub const CONTEXT_CHECKPOINT_EVENT_KIND: &str = "renoa.agent.context-checkpoint.v1";
const CONTEXT_CHECKPOINT_EVENT_PREFIX: &str = "renoa.agent.context-checkpoint.";

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

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "phase", rename_all = "snake_case", deny_unknown_fields)]
pub(crate) enum LoopPhase {
    NeedModel {
        model_turns: u32,
    },
    AwaitingModel {
        model_turns: u32,
    },
    AwaitingCompaction {
        model_turns: u32,
        plan: CompactionPlan,
        max_attempts: NonZeroU32,
        attempt: NonZeroU32,
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

#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case", deny_unknown_fields)]
pub(crate) enum ModelEffectOutput {
    Completed { response: ModelResponse },
    ContextWindowExceeded { message: String },
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ContextCheckpointEvent {
    covered_through_sequence: u64,
    summary: String,
}

pub(crate) fn checkpoint(phase: LoopPhase) -> Result<Checkpoint, LoopError> {
    serde_json::to_value(phase)
        .map(|state| Checkpoint::new(CHECKPOINT_SCHEMA_VERSION, state))
        .map_err(|error| LoopError::new(format!("agent checkpoint encoding failed: {error}")))
}

pub(crate) fn decode_checkpoint(checkpoint: &Checkpoint) -> Result<LoopPhase, LoopError> {
    let phase = serde_json::from_value(checkpoint.state().clone())
        .map_err(|error| LoopError::new(format!("agent checkpoint is invalid: {error}")))?;
    if let LoopPhase::AwaitingCompaction {
        max_attempts,
        attempt,
        ..
    } = &phase
        && attempt > max_attempts
    {
        return Err(LoopError::new(
            "agent checkpoint compaction attempt exceeds its maximum",
        ));
    }
    Ok(phase)
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

pub(crate) fn context_checkpoint_event(
    covered_through_sequence: u64,
    summary: String,
) -> Result<NewEvent, LoopError> {
    if summary.trim().is_empty() {
        return Err(LoopError::new(
            "activated context checkpoint summary cannot be empty",
        ));
    }
    serde_json::to_value(ContextCheckpointEvent {
        covered_through_sequence,
        summary,
    })
    .map(|payload| NewEvent::new(CONTEXT_CHECKPOINT_EVENT_KIND, payload))
    .map_err(|error| LoopError::new(format!("context checkpoint encoding failed: {error}")))
}

pub(crate) fn context_input(
    active_operation_id: renoa_kernel::OperationId,
    events: &[SemanticEvent],
    system_prompt: &str,
    tools: &[ToolSpec],
    compaction_required: bool,
) -> Result<ContextInput, LoopError> {
    let mut entries = Vec::new();
    let mut message_sequences = HashSet::new();
    let mut checkpoint: Option<ActivatedCheckpoint> = None;
    for event in events {
        if event.kind == MESSAGE_EVENT_KIND {
            let message = serde_json::from_value(event.payload.clone()).map_err(|error| {
                LoopError::new(format!(
                    "message event {} cannot be decoded: {error}",
                    event.event_id
                ))
            })?;
            if !message_sequences.insert(event.sequence) {
                return Err(LoopError::new(format!(
                    "message event sequence {} is duplicated",
                    event.sequence
                )));
            }
            entries.push((
                ContextOrigin::new(event.operation_id, event.sequence),
                message,
            ));
        } else if event.kind.starts_with(MESSAGE_EVENT_PREFIX) {
            return Err(LoopError::new(format!(
                "message event kind `{}` is unsupported",
                event.kind
            )));
        } else if event.kind == CONTEXT_CHECKPOINT_EVENT_KIND {
            let decoded = serde_json::from_value::<ContextCheckpointEvent>(event.payload.clone())
                .map_err(|error| {
                LoopError::new(format!(
                    "context checkpoint event {} cannot be decoded: {error}",
                    event.event_id
                ))
            })?;
            if decoded.summary.trim().is_empty() {
                return Err(LoopError::new(
                    "context checkpoint event has an empty summary",
                ));
            }
            if !message_sequences.contains(&decoded.covered_through_sequence) {
                return Err(LoopError::new(
                    "context checkpoint boundary is not an earlier durable message",
                ));
            }
            if checkpoint.as_ref().is_some_and(|current| {
                current.covered_through_sequence >= decoded.covered_through_sequence
            }) {
                return Err(LoopError::new(
                    "context checkpoint does not advance its durable message boundary",
                ));
            }
            checkpoint = Some(ActivatedCheckpoint {
                covered_through_sequence: decoded.covered_through_sequence,
                summary: decoded.summary,
            });
        } else if event.kind.starts_with(CONTEXT_CHECKPOINT_EVENT_PREFIX) {
            return Err(LoopError::new(format!(
                "context checkpoint event kind `{}` is unsupported",
                event.kind
            )));
        }
    }
    Ok(ContextInput::new(
        active_operation_id,
        entries,
        checkpoint,
        system_prompt,
        tools,
        compaction_required,
    ))
}
