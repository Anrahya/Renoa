use std::num::NonZeroU32;

use renoa_agent::{Message, ModelResponse, ToolSpec};
use renoa_kernel::OperationId;
use thiserror::Error;

use crate::compaction::{CompactionCheckpoint, CompactionPlan};

#[derive(Debug, Clone, Copy)]
pub(crate) struct ContextOrigin {
    operation_id: OperationId,
    sequence: u64,
}

impl ContextOrigin {
    pub(crate) const fn new(operation_id: OperationId, sequence: u64) -> Self {
        Self {
            operation_id,
            sequence,
        }
    }
}

/// One durable message plus its session-journal position.
#[derive(Debug, Clone, Copy)]
pub struct ContextEntry<'a> {
    origin: ContextOrigin,
    message: &'a Message,
}

impl ContextEntry<'_> {
    /// Returns the operation that produced this message.
    #[must_use]
    pub const fn operation_id(&self) -> OperationId {
        self.origin.operation_id
    }

    /// Returns the message's gapless session-local event sequence.
    #[must_use]
    pub const fn sequence(&self) -> u64 {
        self.origin.sequence
    }

    /// Returns the provider-neutral durable message.
    #[must_use]
    pub const fn message(&self) -> &Message {
        self.message
    }
}

/// Durable messages available when preparing one model request.
///
/// The loop owns construction of this input. Private fields let the contract
/// grow with concrete context needs without making strategy implementations
/// construct kernel state themselves.
#[derive(Debug)]
pub struct ContextInput {
    active_operation_id: OperationId,
    origins: Vec<ContextOrigin>,
    messages: Vec<Message>,
    checkpoint: Option<ActivatedCheckpoint>,
    system_prompt: String,
    tools: Vec<ToolSpec>,
    compaction_required: bool,
}

impl ContextInput {
    pub(crate) fn new(
        active_operation_id: OperationId,
        entries: Vec<(ContextOrigin, Message)>,
        checkpoint: Option<ActivatedCheckpoint>,
        system_prompt: &str,
        tools: &[ToolSpec],
        compaction_required: bool,
    ) -> Self {
        let (origins, messages) = entries.into_iter().unzip();
        Self {
            active_operation_id,
            origins,
            messages,
            checkpoint,
            system_prompt: system_prompt.to_owned(),
            tools: tools.to_vec(),
            compaction_required,
        }
    }

    /// Returns the operation being prepared for its next model call.
    #[must_use]
    pub const fn active_operation_id(&self) -> OperationId {
        self.active_operation_id
    }

    /// Returns the complete decoded session transcript.
    #[must_use]
    pub fn messages(&self) -> &[Message] {
        &self.messages
    }

    /// Iterates over messages with their durable operation and sequence.
    #[must_use]
    pub fn entries(&self) -> impl DoubleEndedIterator<Item = ContextEntry<'_>> + ExactSizeIterator {
        self.origins
            .iter()
            .copied()
            .zip(&self.messages)
            .map(|(origin, message)| ContextEntry { origin, message })
    }

    /// Returns the latest durably activated portable summary, when present.
    #[must_use]
    pub fn active_checkpoint(&self) -> Option<CompactionCheckpoint<'_>> {
        self.checkpoint.as_ref().map(|checkpoint| {
            CompactionCheckpoint::new(checkpoint.covered_through_sequence, &checkpoint.summary)
        })
    }

    /// Returns the frozen system instructions for the candidate model request.
    #[must_use]
    pub fn system_prompt(&self) -> &str {
        &self.system_prompt
    }

    /// Returns the frozen tool specifications for the candidate model request.
    #[must_use]
    pub fn tools(&self) -> &[ToolSpec] {
        &self.tools
    }

    /// Reports a definite provider rejection that requires compaction even
    /// when the configured estimator considered the request dispatchable.
    #[must_use]
    pub const fn compaction_required(&self) -> bool {
        self.compaction_required
    }

    /// Takes ownership of the complete decoded session transcript.
    #[must_use]
    pub fn into_messages(self) -> Vec<Message> {
        self.messages
    }
}

#[derive(Debug)]
pub(crate) struct ActivatedCheckpoint {
    pub(crate) covered_through_sequence: u64,
    pub(crate) summary: String,
}

/// The pure context decision made before one persisted model effect.
#[derive(Debug, PartialEq)]
#[non_exhaustive]
pub enum ContextPreparation {
    /// Dispatch a normal model request with these exact ordered messages.
    Model { messages: Vec<Message> },
    /// Persist and execute this summary request before another normal request.
    Compact {
        plan: CompactionPlan,
        max_attempts: NonZeroU32,
    },
    /// No safe summary prefix can make the current request dispatchable.
    CapacityExceeded {
        estimated_input_tokens: u64,
        dispatch_limit_tokens: u64,
    },
}

/// Pure decision for one explicit, model-free-after-summary compaction turn.
#[derive(Debug, PartialEq)]
#[non_exhaustive]
pub enum ExplicitCompactionPreparation {
    /// The active checkpoint already covers every durable conversation message.
    UpToDate { estimated_input_tokens: u64 },
    /// Persist and execute this summary request, then finish the control turn.
    Compact {
        plan: CompactionPlan,
        max_attempts: NonZeroU32,
    },
    /// No safe summary prefix can be sent within the configured provider limit.
    CapacityExceeded {
        estimated_input_tokens: u64,
        dispatch_limit_tokens: u64,
    },
}

/// Pure policy for selecting the messages visible to the next model call.
///
/// A strategy must be deterministic for the same input and binding revision.
/// It cannot perform external work; model-backed compaction must use a
/// kernel-dispatched effect. Projection changes only the model-facing view and
/// never deletes or rewrites the durable semantic event journal.
pub trait ContextStrategy: Send + Sync {
    /// Produces the ordered messages for the next model request.
    ///
    /// # Errors
    ///
    /// A failure commits no loop transition and can be retried safely.
    fn project(&self, input: ContextInput) -> Result<Vec<Message>, ContextStrategyError>;

    /// Chooses a normal model view or a durable compaction plan.
    ///
    /// The default preserves projection-only strategies. Implementations that
    /// return [`ContextPreparation::Compact`] must also validate their own
    /// completed summary responses through [`Self::validate_compaction`].
    ///
    /// # Errors
    ///
    /// A failure commits no loop transition and can be retried safely.
    fn prepare(&self, input: ContextInput) -> Result<ContextPreparation, ContextStrategyError> {
        self.project(input)
            .map(|messages| ContextPreparation::Model { messages })
    }

    /// Plans a user-requested compaction turn without inventing a user message.
    ///
    /// # Errors
    ///
    /// Returns an error when this strategy does not support explicit compaction
    /// or cannot safely project the durable transcript.
    fn prepare_explicit_compaction(
        &self,
        _input: &ContextInput,
    ) -> Result<ExplicitCompactionPreparation, ContextStrategyError> {
        Err(ContextStrategyError::new(
            "context strategy does not support explicit compaction",
        ))
    }

    /// Estimates the exact idle model context after activating `summary` for `plan`.
    ///
    /// # Errors
    ///
    /// Returns an error when the plan cannot be projected by this strategy.
    fn estimate_after_explicit_compaction(
        &self,
        _input: &ContextInput,
        _plan: &CompactionPlan,
        _summary: &str,
    ) -> Result<u64, ContextStrategyError> {
        Err(ContextStrategyError::new(
            "context strategy cannot estimate explicit compaction",
        ))
    }

    /// Validates one complete response to a plan produced by this strategy.
    ///
    /// # Errors
    ///
    /// Rejection consumes one bounded compaction attempt but activates no
    /// checkpoint.
    fn validate_compaction(
        &self,
        _plan: &CompactionPlan,
        _response: &ModelResponse,
        _system_prompt: &str,
        _tools: &[ToolSpec],
    ) -> Result<String, CompactionValidationError> {
        Err(CompactionValidationError::new(
            "context strategy does not support compaction responses",
        ))
    }
}

/// A deterministic transformation applied to normal model-request messages.
///
/// This is the composition point for host-owned message projection around the
/// built-in durable compaction policy. The same projector is applied before a
/// normal request is sized and to every retained-tail candidate considered by
/// the compaction planner. Summary requests remain isolated and tool-free. A
/// projector may filter, replace, or add messages, but it must preserve the
/// [`crate::ContextSizer`] monotonicity contract: extending its supplied
/// candidate without removing messages cannot reduce the projected estimate.
/// Any projector configuration must be represented by the surrounding
/// [`crate::ContextBinding`] revision.
pub trait ContextProjector: Send + Sync {
    /// Transforms one complete candidate message set without external work.
    ///
    /// # Errors
    ///
    /// A failure commits no loop transition and can be retried safely.
    fn project(&self, messages: Vec<Message>) -> Result<Vec<Message>, ContextStrategyError>;
}

pub(crate) fn model_visible_messages(mut messages: Vec<Message>) -> Vec<Message> {
    for message in &mut messages {
        if let Message::Tool { result } = message {
            result.details = None;
        }
    }
    messages
}

/// The built-in strategy that exposes every durable message in order.
#[derive(Debug, Clone, Copy, Default)]
pub struct FullHistoryStrategy;

impl ContextStrategy for FullHistoryStrategy {
    fn project(&self, input: ContextInput) -> Result<Vec<Message>, ContextStrategyError> {
        Ok(input.into_messages())
    }
}

/// A context strategy could not produce a valid model-facing view.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
#[error("{message}")]
pub struct ContextStrategyError {
    message: String,
}

impl ContextStrategyError {
    /// Creates a strategy error suitable for host diagnostics.
    #[must_use]
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }
}

/// A completed compaction response was unsafe to activate.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
#[error("{message}")]
pub struct CompactionValidationError {
    message: String,
}

impl CompactionValidationError {
    /// Creates a validation error suitable for bounded retry diagnostics.
    #[must_use]
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }
}
