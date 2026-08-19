use renoa_agent::Message;
use renoa_kernel::OperationId;
use thiserror::Error;

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
}

impl ContextInput {
    pub(crate) fn new(
        active_operation_id: OperationId,
        entries: Vec<(ContextOrigin, Message)>,
    ) -> Self {
        let (origins, messages) = entries.into_iter().unzip();
        Self {
            active_operation_id,
            origins,
            messages,
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

    /// Takes ownership of the complete decoded session transcript.
    #[must_use]
    pub fn into_messages(self) -> Vec<Message> {
        self.messages
    }
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
