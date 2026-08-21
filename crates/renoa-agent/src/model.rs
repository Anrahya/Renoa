use std::collections::BTreeMap;

use futures_util::stream::BoxStream;
use serde::{Deserialize, Serialize};
use thiserror::Error;
use tokio_util::sync::CancellationToken;

use crate::{AssistantContent, AssistantMetadata, Message, ToolSpec};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModelRequest {
    pub system_prompt: String,
    pub messages: Vec<Message>,
    pub tools: Vec<ToolSpec>,
}

/// Why a provider successfully ended one assistant response.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StopReason {
    Stop,
    ToolUse,
    Length,
}

/// Provider-reported tokens normalized without pricing policy.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct TokenUsage {
    /// Input tokens not served from or written to a provider cache.
    pub input: u64,
    /// Generated tokens, including reasoning tokens when the provider counts them here.
    pub output: u64,
    /// Input tokens served from a provider cache.
    pub cache_read: u64,
    /// Input tokens written to a provider cache.
    pub cache_write: u64,
}

impl TokenUsage {
    pub(crate) fn add(&mut self, usage: Self) {
        self.input += usage.input;
        self.output += usage.output;
        self.cache_read += usage.cache_read;
        self.cache_write += usage.cache_write;
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModelResponse {
    pub content: Vec<AssistantContent>,
    pub stop_reason: StopReason,
    /// `None` means the provider did not report enough data to count this turn.
    pub usage: Option<TokenUsage>,
    pub metadata: AssistantMetadata,
}

/// Provider-neutral transient output for one assistant content block.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum AssistantDelta {
    /// Visible assistant text.
    Text { text: String },
    /// Model reasoning that a host may render separately from visible text.
    Reasoning { text: String },
    /// Stable identity needed before incremental tool arguments can be rendered.
    ToolCallStart { id: String, name: String },
    /// One fragment of the tool call's JSON arguments.
    ToolCallArguments { json_delta: String },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ModelEvent {
    /// Exact provider payload after adapter translation and before dispatch.
    ProviderRequest {
        payload: serde_json::Value,
    },
    /// Redacted transport metadata received before the response body is consumed.
    ProviderResponse {
        status: u16,
        headers: BTreeMap<String, String>,
    },
    /// Transient content for one position in the final assistant content array.
    ContentDelta {
        content_index: usize,
        delta: AssistantDelta,
    },
    Completed {
        response: ModelResponse,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Error)]
#[error("{message}")]
pub struct ModelError {
    kind: ModelErrorKind,
    message: String,
}

impl ModelError {
    #[must_use]
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            kind: ModelErrorKind::OutcomeUnknown,
            message: message.into(),
        }
    }

    /// Reports a provider rejection that is known to have happened before inference.
    #[must_use]
    pub fn context_window_exceeded(message: impl Into<String>) -> Self {
        Self {
            kind: ModelErrorKind::ContextWindowExceeded,
            message: message.into(),
        }
    }

    /// Reports a credential rejection that is known to have happened before inference.
    #[must_use]
    pub fn authentication_failed(message: impl Into<String>) -> Self {
        Self {
            kind: ModelErrorKind::AuthenticationFailed,
            message: message.into(),
        }
    }

    /// Reports a model deadline whose provider-side outcome is not proven.
    #[must_use]
    pub fn timeout(message: impl Into<String>) -> Self {
        Self {
            kind: ModelErrorKind::Timeout,
            message: message.into(),
        }
    }

    #[must_use]
    pub fn kind(&self) -> ModelErrorKind {
        self.kind
    }
}

/// What the caller can safely infer about a failed model invocation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum ModelErrorKind {
    /// The provider may have dispatched or generated the response.
    OutcomeUnknown,
    /// The provider rejected the request for exceeding its context window before inference.
    ContextWindowExceeded,
    /// Credential resolution failed before the provider began inference.
    AuthenticationFailed,
    /// A first-event, idle, or total deadline expired.
    Timeout,
}

impl ModelErrorKind {
    pub(crate) const fn is_known_before_inference(self) -> bool {
        matches!(
            self,
            Self::ContextWindowExceeded | Self::AuthenticationFailed
        )
    }
}

pub type ModelEventStream<'a> = BoxStream<'a, Result<ModelEvent, ModelError>>;

pub trait Model: Send + Sync {
    /// Starts one provider invocation.
    ///
    /// The returned stream owns the invocation. When `cancellation` fires it
    /// must stop all started work, including descendant processes, and close
    /// only after cleanup is complete. Dropping the stream must also initiate
    /// cleanup; detached provider work is forbidden.
    fn stream(
        &self,
        request: ModelRequest,
        cancellation: CancellationToken,
    ) -> ModelEventStream<'_>;
}
