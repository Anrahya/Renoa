//! A tool result that ends the turn ([`ToolResult::closing`]).
//!
//! The loop appends the result, gives every later call of the same response
//! the tool's `not_run` text, and appends the closing message as the turn's
//! last assistant message, so a surface shows it as the reply. The turn then
//! waits for input: the next command answers it.

use renoa_agent::{AssistantContent, AssistantMetadata, Closing, Message, StopReason, ToolResult};
use renoa_kernel::{LoopDecision, LoopError};

use super::unavailable_result;
use crate::{
    format::{LoopPhase, checkpoint, message_events},
    pending_tools::PendingToolCalls,
};

pub(super) fn close_turn(
    pending: &PendingToolCalls,
    result: ToolResult,
    closing: Closing,
) -> Result<LoopDecision, LoopError> {
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
        metadata: AssistantMetadata::default(),
    };
    let messages = std::iter::once(Message::Tool { result })
        .chain(not_run)
        .chain(std::iter::once(reply));
    Ok(LoopDecision::WaitForInput {
        checkpoint: checkpoint(LoopPhase::Terminal)?,
        events: message_events(messages)?,
    })
}
