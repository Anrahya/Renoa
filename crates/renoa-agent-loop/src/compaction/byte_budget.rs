use std::{io::Write, num::NonZeroU64};

use renoa_agent::{
    AssistantContent, AssistantMetadata, Closing, ContentBlock, Message, ModelRequest, StopReason,
    TokenUsage, ToolResult, ToolSpec,
};
use serde::Serialize;

/// Serialized JSON request byte thresholds enforced alongside token limits.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RequestByteBudget {
    dispatch_limit: NonZeroU64,
    automatic_compaction: NonZeroU64,
    tail_budget: NonZeroU64,
}

impl RequestByteBudget {
    pub(super) const fn new(
        dispatch_limit: NonZeroU64,
        automatic_compaction: NonZeroU64,
        tail_budget: NonZeroU64,
    ) -> Self {
        Self {
            dispatch_limit,
            automatic_compaction,
            tail_budget,
        }
    }

    /// Returns the largest model-visible serialized request accepted for dispatch.
    #[must_use]
    pub const fn dispatch_limit_bytes(self) -> NonZeroU64 {
        self.dispatch_limit
    }

    /// Returns the serialized request byte size that triggers automatic compaction.
    #[must_use]
    pub const fn automatic_compaction_bytes(self) -> NonZeroU64 {
        self.automatic_compaction
    }

    /// Returns the retained-tail serialized byte budget used during compaction.
    #[must_use]
    pub const fn tail_budget_bytes(self) -> NonZeroU64 {
        self.tail_budget
    }
}

/// Counts the exact serialized JSON bytes of `request` with host-only
/// [`ToolResult::details`](renoa_agent::ToolResult::details) omitted, matching
/// the model-visible wire request without cloning message payloads.
pub(super) fn request_visible_json_bytes(request: &ModelRequest) -> Result<u64, serde_json::Error> {
    let ModelRequest {
        system_prompt,
        messages,
        tools,
    } = request;
    let visible = VisibleModelRequest {
        system_prompt,
        messages: messages.iter().map(VisibleMessage::from).collect(),
        tools,
    };
    let mut counter = ByteCounter(0);
    serde_json::to_writer(&mut counter, &visible)?;
    Ok(counter.0)
}

#[derive(Serialize)]
struct VisibleModelRequest<'a> {
    system_prompt: &'a str,
    messages: Vec<VisibleMessage<'a>>,
    tools: &'a [ToolSpec],
}

#[derive(Serialize)]
#[serde(tag = "role", rename_all = "snake_case")]
enum VisibleMessage<'a> {
    User {
        content: &'a [ContentBlock],
    },
    Assistant {
        content: &'a [AssistantContent],
        stop_reason: StopReason,
        usage: Option<TokenUsage>,
        metadata: &'a AssistantMetadata,
    },
    Tool {
        result: VisibleToolResult<'a>,
    },
}

#[derive(Serialize)]
struct VisibleToolResult<'a> {
    call_id: &'a str,
    name: &'a str,
    content: &'a [ContentBlock],
    details: Option<()>,
    is_error: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    closing: &'a Option<Closing>,
}

impl<'a> From<&'a Message> for VisibleMessage<'a> {
    fn from(message: &'a Message) -> Self {
        match message {
            Message::User { content } => Self::User { content },
            Message::Assistant {
                content,
                stop_reason,
                usage,
                metadata,
            } => Self::Assistant {
                content,
                stop_reason: *stop_reason,
                usage: *usage,
                metadata,
            },
            Message::Tool { result } => {
                let ToolResult {
                    call_id,
                    name,
                    content,
                    details: _,
                    is_error,
                    closing,
                } = result;
                Self::Tool {
                    result: VisibleToolResult {
                        call_id,
                        name,
                        content,
                        details: None,
                        is_error: *is_error,
                        closing,
                    },
                }
            }
        }
    }
}

struct ByteCounter(u64);

impl Write for ByteCounter {
    fn write(&mut self, buffer: &[u8]) -> std::io::Result<usize> {
        self.0 = self
            .0
            .saturating_add(u64::try_from(buffer.len()).unwrap_or(u64::MAX));
        Ok(buffer.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}
