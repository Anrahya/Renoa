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
    /// Adapter retry diagnostic. Never enters model context.
    RetryAttempt {
        attempt: u32,
        next_attempt: u32,
        category: ModelErrorKind,
        delay_ms: u64,
        cause_code: Option<String>,
    },
    Completed {
        response: ModelResponse,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Error)]
#[error("{message}")]
pub struct ModelError {
    kind: ModelErrorKind,
    inference_outcome: InferenceOutcome,
    message: String,
    diagnostic: Option<Box<ModelFailureDiagnostic>>,
}

impl ModelError {
    #[must_use]
    pub fn new(message: impl Into<String>) -> Self {
        Self::classified(
            ModelErrorKind::Unknown,
            InferenceOutcome::Unknown,
            message,
            None,
        )
    }

    /// Builds a classified provider failure that the loop and traces can consume.
    #[must_use]
    pub fn classified(
        kind: ModelErrorKind,
        inference_outcome: InferenceOutcome,
        message: impl Into<String>,
        diagnostic: Option<ModelFailureDiagnostic>,
    ) -> Self {
        Self {
            kind,
            inference_outcome,
            message: message.into(),
            diagnostic: diagnostic.map(Box::new),
        }
    }

    /// Reports a provider rejection that is known to have happened before inference.
    #[must_use]
    pub fn context_window_exceeded(message: impl Into<String>) -> Self {
        Self::classified(
            ModelErrorKind::ContextWindowExceeded,
            InferenceOutcome::KnownNotStarted,
            message,
            None,
        )
    }

    /// Reports a credential rejection that is known to have happened before inference.
    #[must_use]
    pub fn authentication_failed(message: impl Into<String>) -> Self {
        Self::classified(
            ModelErrorKind::Authentication,
            InferenceOutcome::KnownNotStarted,
            message,
            None,
        )
    }

    /// Reports a model deadline whose provider-side outcome is not proven.
    #[must_use]
    pub fn timeout(message: impl Into<String>) -> Self {
        Self::classified(
            ModelErrorKind::Timeout,
            InferenceOutcome::Unknown,
            message,
            None,
        )
    }

    /// Reports a caller-requested cancellation.
    #[must_use]
    pub fn cancelled(message: impl Into<String>) -> Self {
        Self::classified(
            ModelErrorKind::Cancelled,
            InferenceOutcome::KnownNotStarted,
            message,
            None,
        )
    }

    #[must_use]
    pub const fn kind(&self) -> ModelErrorKind {
        self.kind
    }

    #[must_use]
    pub const fn inference_outcome(&self) -> InferenceOutcome {
        self.inference_outcome
    }

    #[must_use]
    pub fn diagnostic(&self) -> Option<&ModelFailureDiagnostic> {
        self.diagnostic.as_deref()
    }

    #[must_use]
    pub fn with_unknown_outcome(mut self) -> Self {
        self.inference_outcome = InferenceOutcome::Unknown;
        self
    }
}

/// What the caller can safely infer about whether provider inference ran.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum InferenceOutcome {
    /// The provider rejected or never accepted the request.
    KnownNotStarted,
    /// The provider may already have dispatched or generated a response.
    Unknown,
}

/// Provider-neutral failure category for one unsuccessful model invocation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum ModelErrorKind {
    Authentication,
    RateLimited,
    InvalidRequest,
    ContextWindowExceeded,
    Network,
    Timeout,
    ProviderUnavailable,
    Protocol,
    StreamInterrupted,
    Cancelled,
    Unknown,
}

/// Redacted transport facts that traces and ACP diagnostics may consume.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModelFailureDiagnostic {
    pub provider: String,
    pub model: String,
    pub attempt_count: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub http_status: Option<u16>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub provider_code: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub request_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub retry_after: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cause_code: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cause_message: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub provider_message: Option<String>,
}

pub type ModelEventStream<'a> = BoxStream<'a, Result<ModelEvent, ModelError>>;

pub trait Model: Send + Sync {
    /// Starts one logical model effect for an exact provider-neutral request.
    ///
    /// An adapter may expose bounded pre-output retries as [`ModelEvent::RetryAttempt`]
    /// under its documented policy; they remain part of this one effect and may
    /// not be hidden after assistant output starts. The returned stream owns all
    /// attempts. When `cancellation` fires it must stop all started work,
    /// including descendant processes, and close only after cleanup is
    /// complete. Dropping the stream must also initiate cleanup; detached
    /// provider work is forbidden.
    fn stream(
        &self,
        request: ModelRequest,
        cancellation: CancellationToken,
    ) -> ModelEventStream<'_>;
}
