//! A tool result that ends the turn ([`ToolResult::closing`]).
//!
//! The loop appends the result, gives every later call of the same response
//! the tool's `not_run` text, and appends the closing message as the turn's
//! last assistant message, so a surface shows it as the reply. The message
//! carries the provider metadata of the assistant message that made the call,
//! as every assistant message in history must. The turn then waits for input:
//! the next command answers it.

use std::collections::BTreeMap;

use renoa_agent::{
    AssistantContent, AssistantMetadata, Closing, ContentBlock, Message, StopReason, ToolResult,
};
use renoa_kernel::{LoopDecision, LoopError, SemanticEvent};

use super::unavailable_result;
use crate::{
    code_mode::{MAX_CODE_RESULT_BYTES, model_result},
    format::{LoopPhase, MESSAGE_EVENT_KIND, checkpoint, message_events},
    pending_tools::PendingToolCalls,
};

pub(super) fn close_turn(
    pending: &PendingToolCalls,
    result: ToolResult,
    closing: Closing,
    events: &[SemanticEvent],
) -> Result<LoopDecision, LoopError> {
    if closing.message.trim().is_empty() {
        return Err(LoopError::new(format!(
            "tool `{}` ended the turn with an empty closing message",
            result.name
        )));
    }
    let not_run = pending.remaining().map(|call| Message::Tool {
        result: unavailable_result(call, &closing.not_run),
    });
    let reply = Message::Assistant {
        content: vec![AssistantContent::Text {
            text: closing.message,
            signature: None,
        }],
        stop_reason: StopReason::Stop,
        usage: None,
        metadata: caller_metadata(events)?,
    };
    let messages = std::iter::once(Message::Tool { result })
        .chain(not_run)
        .chain(std::iter::once(reply));
    Ok(LoopDecision::WaitForInput {
        checkpoint: checkpoint(LoopPhase::Terminal)?,
        events: message_events(messages)?,
    })
}

/// Stops a Code Mode run whose call wave ended the turn: Python never
/// resumes. The outer `code_mode` result holds what each call of the wave
/// returned, keyed by its call id, without details, or only that the run
/// stopped when that passes the final-result limit. Every closing message of
/// the wave is the reply, in call order.
pub(super) fn stop_code_run(
    pending: &PendingToolCalls,
    wave: &BTreeMap<String, (Vec<ContentBlock>, bool)>,
    closings: Vec<Closing>,
    events: &[SemanticEvent],
) -> Result<LoopDecision, LoopError> {
    let results: BTreeMap<_, _> = wave
        .iter()
        .map(|(id, (content, is_error))| {
            (
                id,
                serde_json::json!({"content": content, "is_error": is_error}),
            )
        })
        .collect();
    let mut stopped = serde_json::json!({"stopped": "a call ended the turn", "results": results});
    let bytes = serde_json::to_vec(&stopped)
        .map_err(|error| LoopError::new(format!("Code Mode stop encoding failed: {error}")))?
        .len();
    if bytes > MAX_CODE_RESULT_BYTES {
        stopped = serde_json::json!({"stopped": "a call ended the turn"});
    }
    let result = model_result(pending.current(), &stopped, false)?;
    let not_run = closings
        .first()
        .map(|closing| closing.not_run.clone())
        .unwrap_or_default();
    let message = closings
        .into_iter()
        .map(|closing| closing.message)
        .collect::<Vec<_>>()
        .join("\n\n");
    close_turn(pending, result, Closing { message, not_run }, events)
}

/// The provider, API and model of the latest assistant message: the one whose
/// tool call is settling, since only tool results follow it.
fn caller_metadata(events: &[SemanticEvent]) -> Result<AssistantMetadata, LoopError> {
    for event in events.iter().rev() {
        if event.kind != MESSAGE_EVENT_KIND {
            continue;
        }
        let message: Message = serde_json::from_value(event.payload.clone())
            .map_err(|error| LoopError::new(format!("history message is invalid: {error}")))?;
        if let Message::Assistant { metadata, .. } = message {
            return Ok(AssistantMetadata {
                api: metadata.api,
                provider: metadata.provider,
                model: metadata.model,
                ..AssistantMetadata::default()
            });
        }
    }
    Err(LoopError::new(
        "a closing tool result has no assistant message before it",
    ))
}
