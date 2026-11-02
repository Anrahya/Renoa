//! How a finished receipted operation shows in a later request: as one user
//! message, the receipt its context strategy renders from the operation's own
//! messages, in place of its whole exchange.
//!
//! The journal keeps every message. The receipt takes the sequence of the
//! operation's last message, so a checkpoint or a cut at the operation's end
//! still names a message in view; a checkpoint that ended inside the operation
//! moves to that end, since the summary it activated already covers the part
//! before it. The operation being prepared always sees its own exchange.

use std::collections::{HashMap, HashSet};

use renoa_agent::Message;
use renoa_kernel::{LoopError, OperationId, SemanticEvent};

use super::{RECEIPTED_EVENT_KIND, RECEIPTED_EVENT_PREFIX};
use crate::{
    context::{ActivatedCheckpoint, ContextOrigin, ContextStrategy},
    turn_context::TurnAnnotation,
};

/// Whether `event` is a receipted marker, recording its operation in
/// `receipted`. An operation is marked once, and only this version is known.
pub(super) fn mark(
    event: &SemanticEvent,
    receipted: &mut HashSet<OperationId>,
) -> Result<bool, LoopError> {
    if event.kind == RECEIPTED_EVENT_KIND {
        if !receipted.insert(event.operation_id) {
            return Err(LoopError::new(format!(
                "operation {} is marked receipted more than once",
                event.operation_id
            )));
        }
        return Ok(true);
    }
    if event.kind.starts_with(RECEIPTED_EVENT_PREFIX) {
        return Err(LoopError::new(format!(
            "receipted event kind `{}` is unsupported",
            event.kind
        )));
    }
    Ok(false)
}

/// Refuses a receipted operation with turn timing or context: a receipted
/// prompt carries neither.
pub(super) fn unannotated(
    receipted: &HashSet<OperationId>,
    annotations: &HashMap<OperationId, TurnAnnotation>,
) -> Result<(), LoopError> {
    if receipted
        .iter()
        .any(|operation| annotations.contains_key(operation))
    {
        return Err(LoopError::new(
            "a receipted operation cannot carry turn timing or context",
        ));
    }
    Ok(())
}

/// `entries` with each finished receipted operation replaced by its receipt.
/// An operation's messages are contiguous: the kernel runs one at a time.
pub(super) fn replace(
    entries: Vec<(ContextOrigin, Message)>,
    receipted: &HashSet<OperationId>,
    active: OperationId,
    strategy: &dyn ContextStrategy,
    mut checkpoint: Option<&mut ActivatedCheckpoint>,
) -> Result<Vec<(ContextOrigin, Message)>, LoopError> {
    if receipted.iter().all(|operation| *operation == active) {
        return Ok(entries);
    }
    let mut replaced = Vec::with_capacity(entries.len());
    let mut held: Vec<(ContextOrigin, Message)> = Vec::new();
    for entry in entries {
        let operation = entry.0.operation_id();
        if held
            .first()
            .is_some_and(|(origin, _)| origin.operation_id() != operation)
        {
            replaced.push(receipt(
                std::mem::take(&mut held),
                strategy,
                checkpoint.as_deref_mut(),
            )?);
        }
        if operation != active && receipted.contains(&operation) {
            held.push(entry);
        } else {
            replaced.push(entry);
        }
    }
    if !held.is_empty() {
        replaced.push(receipt(held, strategy, checkpoint)?);
    }
    Ok(replaced)
}

/// The receipt of one operation's messages, `held` in order.
fn receipt(
    held: Vec<(ContextOrigin, Message)>,
    strategy: &dyn ContextStrategy,
    checkpoint: Option<&mut ActivatedCheckpoint>,
) -> Result<(ContextOrigin, Message), LoopError> {
    let (Some((first, _)), Some((last, _))) = (held.first(), held.last()) else {
        return Err(LoopError::new("a receipt needs at least one message"));
    };
    let (first, last) = (first.sequence(), *last);
    if let Some(checkpoint) = checkpoint
        && (first..last.sequence()).contains(&checkpoint.covered_through_sequence)
    {
        checkpoint.covered_through_sequence = last.sequence();
    }
    let messages: Vec<Message> = held.into_iter().map(|(_, message)| message).collect();
    let text = strategy.receipt(&messages).map_err(|error| {
        LoopError::new(format!(
            "operation {} has no receipt: {error}",
            last.operation_id()
        ))
    })?;
    Ok((last, Message::user_text(text)))
}
