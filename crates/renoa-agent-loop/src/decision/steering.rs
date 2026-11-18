//! Steers taken into the operation before its next model call.
//!
//! A steer is accepted only where the operation is about to ask the model
//! again: the persisted request of a model call in flight must still match the
//! history, so nothing may join it while that call runs. Each accepted steer
//! becomes a user message of this operation, with its own time and context.

use renoa_agent::Message;
use renoa_kernel::{LoopDecision, LoopError, LoopInput};

use crate::{
    AgentCommand,
    format::{
        AgentCommandKind, LoopPhase, Observation, checkpoint, message_event, observation_event,
    },
};

/// The decision that takes `input`'s pending steers in, if any can be.
///
/// An operation started by a receipted prompt or by a control command takes
/// none: its exchange is replaced or never shown, and the steer would vanish
/// with it. A steer that is not a person's prompt stays pending, so the Host
/// runs it as its own command once this operation ends.
pub(super) fn accept_steers(
    model_turns: u32,
    input: &LoopInput,
) -> Result<Option<LoopDecision>, LoopError> {
    if input.steers.is_empty() || !takes_steers(input)? {
        return Ok(None);
    }
    let mut events = Vec::new();
    let mut accepted = Vec::new();
    for steer in &input.steers {
        let Ok(command) = serde_json::from_value::<AgentCommand>(steer.content().clone()) else {
            continue;
        };
        let AgentCommandKind::Prompt {
            content,
            observation,
        } = command.into_kind()
        else {
            continue;
        };
        if matches!(observation, Observation::Receipted) {
            continue;
        }
        events.push(message_event(Message::User { content })?);
        events.extend(observation_event(observation)?);
        accepted.push(steer.command_id());
    }
    if accepted.is_empty() {
        return Ok(None);
    }
    Ok(Some(LoopDecision::AcceptSteers {
        checkpoint: checkpoint(LoopPhase::NeedModel { model_turns })?,
        events,
        steers: accepted,
    }))
}

fn takes_steers(input: &LoopInput) -> Result<bool, LoopError> {
    let command = serde_json::from_value::<AgentCommand>(input.command.content().clone())
        .map_err(|error| LoopError::new(format!("invalid agent command: {error}")))?;
    Ok(matches!(
        command.into_kind(),
        AgentCommandKind::Prompt { observation, .. } if !matches!(observation, Observation::Receipted)
    ))
}
