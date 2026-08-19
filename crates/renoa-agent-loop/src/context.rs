use renoa_agent::Message;
use thiserror::Error;

/// Durable messages available when preparing one model request.
///
/// The loop owns construction of this input. Private fields let the contract
/// grow with concrete context needs without making strategy implementations
/// construct kernel state themselves.
#[derive(Debug)]
pub struct ContextInput {
    messages: Vec<Message>,
}

impl ContextInput {
    pub(crate) const fn new(messages: Vec<Message>) -> Self {
        Self { messages }
    }

    /// Returns the complete decoded session transcript.
    #[must_use]
    pub fn messages(&self) -> &[Message] {
        &self.messages
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
