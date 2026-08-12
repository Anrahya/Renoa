use thiserror::Error;
use tokio_util::sync::CancellationToken;

use crate::{BoxFuture, Message};

#[derive(Debug, Clone, PartialEq, Eq, Error)]
#[error("{message}")]
pub struct ContextProjectionError {
    message: String,
}

impl ContextProjectionError {
    #[must_use]
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }
}

/// Host-owned projection of durable transcript state into one model request.
pub trait ContextProjector: Send + Sync {
    fn project(
        &self,
        messages: Vec<Message>,
        cancellation: CancellationToken,
    ) -> BoxFuture<'_, Result<Vec<Message>, ContextProjectionError>>;
}
