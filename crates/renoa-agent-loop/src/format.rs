use std::collections::{HashMap, HashSet};

use renoa_agent::{Message, ModelResponse, ToolSpec};
use renoa_kernel::{LoopError, NewEvent, SemanticEvent};
use serde::{Deserialize, Serialize};

use crate::{
    context::{ActivatedCheckpoint, ContextInput, ContextOrigin},
    turn_context::{TurnAnnotation, TurnContext},
};

mod checkpoint;
mod command;
pub(crate) use checkpoint::{LoopPhase, checkpoint, decode_checkpoint};
pub use command::AgentCommand;
pub(crate) use command::{AgentCommandKind, Observation};

#[cfg(test)]
mod tests;

/// Versioned semantic-event kind carrying one provider-neutral message.
pub const MESSAGE_EVENT_KIND: &str = "renoa.agent.message.v1";
const MESSAGE_EVENT_PREFIX: &str = "renoa.agent.message.";
/// Versioned semantic-event kind carrying Host-observed user-turn timing.
/// Only commands admitted before per-message context produce it.
pub const TURN_TIMING_EVENT_KIND: &str = "renoa.agent.turn-timing.v1";
const TURN_TIMING_EVENT_PREFIX: &str = "renoa.agent.turn-timing.";
/// Versioned semantic-event kind carrying the context admitted with one user message.
pub const TURN_CONTEXT_EVENT_KIND: &str = "renoa.agent.turn-context.v1";
const TURN_CONTEXT_EVENT_PREFIX: &str = "renoa.agent.turn-context.";
/// Versioned semantic-event kind carrying one activated portable summary.
pub const CONTEXT_CHECKPOINT_EVENT_KIND: &str = "renoa.agent.context-checkpoint.v1";
const CONTEXT_CHECKPOINT_EVENT_PREFIX: &str = "renoa.agent.context-checkpoint.";
/// Versioned semantic-event kind carrying the durable result of explicit compaction.
pub const COMPACTION_RESULT_EVENT_KIND: &str = "renoa.agent.compaction-result.v1";
const COMPACTION_RESULT_EVENT_PREFIX: &str = "renoa.agent.compaction-result.";

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

/// The event that carries a prompt's observation into the journal, if it has one.
pub(crate) fn observation_event(observation: Observation) -> Result<Option<NewEvent>, LoopError> {
    let (kind, payload) = match observation {
        Observation::Unobserved => return Ok(None),
        Observation::Timed(timing) => (TURN_TIMING_EVENT_KIND, serde_json::to_value(timing)),
        Observation::Observed { context, .. } if context.is_empty() => return Ok(None),
        Observation::Observed { context, .. } => (
            TURN_CONTEXT_EVENT_KIND,
            serde_json::to_value(TurnContextEvent { entries: context }),
        ),
    };
    payload
        .map(|payload| Some(NewEvent::new(kind, payload)))
        .map_err(|error| LoopError::new(format!("{kind} event encoding failed: {error}")))
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct TurnContextEvent {
    entries: TurnContext,
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

/// Durable context size estimated after an explicit compaction command.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CompactionResult {
    estimated_input_tokens: u64,
}

impl CompactionResult {
    #[must_use]
    pub const fn estimated_input_tokens(self) -> u64 {
        self.estimated_input_tokens
    }
}

pub(crate) fn compaction_result_event(estimated_input_tokens: u64) -> Result<NewEvent, LoopError> {
    serde_json::to_value(CompactionResult {
        estimated_input_tokens,
    })
    .map(|payload| NewEvent::new(COMPACTION_RESULT_EVENT_KIND, payload))
    .map_err(|error| LoopError::new(format!("compaction result encoding failed: {error}")))
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
    let mut annotations = HashMap::new();
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
        } else if let Some(annotation) = decode_annotation(event)? {
            if annotations.insert(event.operation_id, annotation).is_some() {
                return Err(LoopError::new(format!(
                    "operation {} has more than one turn timing or context event",
                    event.operation_id
                )));
            }
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
        } else if event.kind != COMPACTION_RESULT_EVENT_KIND
            && event.kind.starts_with(COMPACTION_RESULT_EVENT_PREFIX)
        {
            return Err(LoopError::new(format!(
                "compaction result event kind `{}` is unsupported",
                event.kind
            )));
        } else if event.kind == COMPACTION_RESULT_EVENT_KIND {
            serde_json::from_value::<CompactionResult>(event.payload.clone()).map_err(|error| {
                LoopError::new(format!(
                    "compaction result event {} cannot be decoded: {error}",
                    event.event_id
                ))
            })?;
        }
    }
    finish_context_input(
        active_operation_id,
        entries,
        &annotations,
        checkpoint,
        system_prompt,
        tools,
        compaction_required,
    )
}

fn decode_payload<T: serde::de::DeserializeOwned>(event: &SemanticEvent) -> Result<T, LoopError> {
    serde_json::from_value(event.payload.clone()).map_err(|error| {
        LoopError::new(format!(
            "{} event {} cannot be decoded: {error}",
            event.kind, event.event_id
        ))
    })
}

/// The turn timing or context an event carries; `None` for any other kind.
fn decode_annotation(event: &SemanticEvent) -> Result<Option<TurnAnnotation>, LoopError> {
    if event.kind == TURN_TIMING_EVENT_KIND {
        return Ok(Some(TurnAnnotation::Timing(decode_payload(event)?)));
    }
    if event.kind == TURN_CONTEXT_EVENT_KIND {
        let context = decode_payload::<TurnContextEvent>(event)?.entries;
        return Ok(Some(TurnAnnotation::Context(context)));
    }
    for (prefix, name) in [
        (TURN_TIMING_EVENT_PREFIX, "turn timing"),
        (TURN_CONTEXT_EVENT_PREFIX, "turn context"),
    ] {
        if event.kind.starts_with(prefix) {
            return Err(LoopError::new(format!(
                "{name} event kind `{}` is unsupported",
                event.kind
            )));
        }
    }
    Ok(None)
}

fn finish_context_input(
    active_operation_id: renoa_kernel::OperationId,
    entries: Vec<(ContextOrigin, Message)>,
    annotations: &HashMap<renoa_kernel::OperationId, TurnAnnotation>,
    checkpoint: Option<ActivatedCheckpoint>,
    system_prompt: &str,
    tools: &[ToolSpec],
    compaction_required: bool,
) -> Result<ContextInput, LoopError> {
    validate_annotations(&entries, annotations)?;
    Ok(ContextInput::new(
        active_operation_id,
        entries,
        annotations,
        checkpoint,
        system_prompt,
        tools,
        compaction_required,
    ))
}

fn validate_annotations(
    entries: &[(ContextOrigin, Message)],
    annotations: &HashMap<renoa_kernel::OperationId, TurnAnnotation>,
) -> Result<(), LoopError> {
    for operation_id in annotations.keys() {
        let user_messages = entries
            .iter()
            .filter(|(origin, message)| {
                origin.operation_id() == *operation_id && matches!(message, Message::User { .. })
            })
            .count();
        if user_messages != 1 {
            return Err(LoopError::new(format!(
                "operation {operation_id} turn timing or context does not belong to exactly one user message"
            )));
        }
    }
    Ok(())
}
