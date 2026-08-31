use std::{
    collections::{HashMap, HashSet},
    num::NonZeroU32,
};

use renoa_agent::{ContentBlock, Message, ModelResponse, ToolSpec};
use renoa_kernel::{Checkpoint, LoopError, NewEvent, SemanticEvent};
use serde::{Deserialize, Deserializer, Serialize, Serializer};

use crate::{
    CompactionPlan,
    configuration::CHECKPOINT_SCHEMA_VERSION,
    context::{ActivatedCheckpoint, ContextInput, ContextOrigin},
    turn_timing::TurnTiming,
};

#[cfg(test)]
mod tests;

/// Versioned semantic-event kind carrying one provider-neutral message.
pub const MESSAGE_EVENT_KIND: &str = "renoa.agent.message.v1";
const MESSAGE_EVENT_PREFIX: &str = "renoa.agent.message.";
/// Versioned semantic-event kind carrying Host-observed user-turn timing.
pub const TURN_TIMING_EVENT_KIND: &str = "renoa.agent.turn-timing.v1";
const TURN_TIMING_EVENT_PREFIX: &str = "renoa.agent.turn-timing.";
/// Versioned semantic-event kind carrying one activated portable summary.
pub const CONTEXT_CHECKPOINT_EVENT_KIND: &str = "renoa.agent.context-checkpoint.v1";
const CONTEXT_CHECKPOINT_EVENT_PREFIX: &str = "renoa.agent.context-checkpoint.";
/// Versioned semantic-event kind carrying the durable result of explicit compaction.
pub const COMPACTION_RESULT_EVENT_KIND: &str = "renoa.agent.compaction-result.v1";
const COMPACTION_RESULT_EVENT_PREFIX: &str = "renoa.agent.compaction-result.";

/// Command content consumed by the model/tool loop.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentCommand {
    kind: AgentCommandKind,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum AgentCommandKind {
    Prompt {
        content: Vec<ContentBlock>,
        turn_timing: Option<TurnTiming>,
    },
    Compact,
}

#[derive(Serialize)]
#[serde(untagged)]
enum AgentCommandRef<'a> {
    Prompt(PromptCommandRef<'a>),
    Control(ControlCommand),
}

#[derive(Serialize)]
#[serde(deny_unknown_fields)]
struct PromptCommandRef<'a> {
    content: &'a [ContentBlock],
    #[serde(skip_serializing_if = "Option::is_none")]
    turn_timing: Option<&'a TurnTiming>,
}

#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum AgentCommandWire {
    Prompt(PromptCommand),
    Control(ControlCommand),
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct PromptCommand {
    content: Vec<ContentBlock>,
    turn_timing: Option<TurnTiming>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ControlCommand {
    control: ControlKind,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum ControlKind {
    Compact,
}

impl AgentCommand {
    #[must_use]
    pub fn new(content: Vec<ContentBlock>) -> Self {
        Self {
            kind: AgentCommandKind::Prompt {
                content,
                turn_timing: None,
            },
        }
    }

    /// Creates a prompt with one Host-observed, durable timing fact.
    #[must_use]
    pub fn timed(content: Vec<ContentBlock>, turn_timing: TurnTiming) -> Self {
        Self {
            kind: AgentCommandKind::Prompt {
                content,
                turn_timing: Some(turn_timing),
            },
        }
    }

    #[must_use]
    pub fn text(text: impl Into<String>) -> Self {
        Self::new(vec![ContentBlock::text(text)])
    }

    #[must_use]
    pub const fn compact() -> Self {
        Self {
            kind: AgentCommandKind::Compact,
        }
    }

    /// Returns the model-visible prompt content, or an empty slice for a
    /// control command that deliberately contributes no conversation message.
    #[must_use]
    pub fn content(&self) -> &[ContentBlock] {
        match &self.kind {
            AgentCommandKind::Prompt { content, .. } => content,
            AgentCommandKind::Compact => &[],
        }
    }

    /// Returns prompt content while preserving the distinction from a control command.
    #[must_use]
    pub fn prompt_content(&self) -> Option<&[ContentBlock]> {
        match &self.kind {
            AgentCommandKind::Prompt { content, .. } => Some(content),
            AgentCommandKind::Compact => None,
        }
    }

    #[must_use]
    pub const fn turn_timing(&self) -> Option<&TurnTiming> {
        match &self.kind {
            AgentCommandKind::Prompt { turn_timing, .. } => turn_timing.as_ref(),
            AgentCommandKind::Compact => None,
        }
    }

    pub(crate) fn into_kind(self) -> AgentCommandKind {
        self.kind
    }
}

impl Serialize for AgentCommand {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        match &self.kind {
            AgentCommandKind::Prompt {
                content,
                turn_timing,
            } => AgentCommandRef::Prompt(PromptCommandRef {
                content,
                turn_timing: turn_timing.as_ref(),
            })
            .serialize(serializer),
            AgentCommandKind::Compact => AgentCommandRef::Control(ControlCommand {
                control: ControlKind::Compact,
            })
            .serialize(serializer),
        }
    }
}

impl<'de> Deserialize<'de> for AgentCommand {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        AgentCommandWire::deserialize(deserializer).map(|wire| match wire {
            AgentCommandWire::Prompt(command) => match command.turn_timing {
                Some(turn_timing) => Self::timed(command.content, turn_timing),
                None => Self::new(command.content),
            },
            AgentCommandWire::Control(ControlCommand {
                control: ControlKind::Compact,
            }) => Self::compact(),
        })
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
    AwaitingExplicitCompaction {
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
    let attempts = match &phase {
        LoopPhase::AwaitingCompaction {
            max_attempts,
            attempt,
            ..
        }
        | LoopPhase::AwaitingExplicitCompaction {
            max_attempts,
            attempt,
            ..
        } => Some((attempt, max_attempts)),
        _ => None,
    };
    if attempts.is_some_and(|(attempt, max_attempts)| attempt > max_attempts) {
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

pub(crate) fn turn_timing_event(turn_timing: TurnTiming) -> Result<NewEvent, LoopError> {
    serde_json::to_value(turn_timing)
        .map(|payload| NewEvent::new(TURN_TIMING_EVENT_KIND, payload))
        .map_err(|error| LoopError::new(format!("turn timing event encoding failed: {error}")))
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
    let mut turn_timings = HashMap::new();
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
        } else if event.kind == TURN_TIMING_EVENT_KIND {
            insert_turn_timing(event, &mut turn_timings)?;
        } else if event.kind.starts_with(TURN_TIMING_EVENT_PREFIX) {
            return Err(LoopError::new(format!(
                "turn timing event kind `{}` is unsupported",
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
        &turn_timings,
        checkpoint,
        system_prompt,
        tools,
        compaction_required,
    )
}

fn insert_turn_timing(
    event: &SemanticEvent,
    turn_timings: &mut HashMap<renoa_kernel::OperationId, TurnTiming>,
) -> Result<(), LoopError> {
    let decoded = serde_json::from_value::<TurnTiming>(event.payload.clone()).map_err(|error| {
        LoopError::new(format!(
            "turn timing event {} cannot be decoded: {error}",
            event.event_id
        ))
    })?;
    if turn_timings.insert(event.operation_id, decoded).is_some() {
        return Err(LoopError::new(format!(
            "operation {} has more than one turn timing event",
            event.operation_id
        )));
    }
    Ok(())
}

fn finish_context_input(
    active_operation_id: renoa_kernel::OperationId,
    entries: Vec<(ContextOrigin, Message)>,
    turn_timings: &HashMap<renoa_kernel::OperationId, TurnTiming>,
    checkpoint: Option<ActivatedCheckpoint>,
    system_prompt: &str,
    tools: &[ToolSpec],
    compaction_required: bool,
) -> Result<ContextInput, LoopError> {
    validate_turn_timings(&entries, turn_timings)?;
    Ok(ContextInput::new(
        active_operation_id,
        entries,
        turn_timings,
        checkpoint,
        system_prompt,
        tools,
        compaction_required,
    ))
}

fn validate_turn_timings(
    entries: &[(ContextOrigin, Message)],
    turn_timings: &HashMap<renoa_kernel::OperationId, TurnTiming>,
) -> Result<(), LoopError> {
    for operation_id in turn_timings.keys() {
        let user_messages = entries
            .iter()
            .filter(|(origin, message)| {
                origin.operation_id() == *operation_id && matches!(message, Message::User { .. })
            })
            .count();
        if user_messages != 1 {
            return Err(LoopError::new(format!(
                "operation {operation_id} turn timing does not belong to exactly one user message"
            )));
        }
    }
    Ok(())
}
