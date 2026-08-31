use thiserror::Error;

use crate::{api::ApiError, store::StoreError};

#[derive(Debug, Error)]
#[non_exhaustive]
pub enum TelegramServiceError {
    #[error("invalid Telegram surface configuration: {0}")]
    Configuration(String),
    #[error("Telegram surface I/O failed: {0}")]
    Io(#[from] std::io::Error),
    #[error(transparent)]
    Api(#[from] ApiError),
    #[error(transparent)]
    Store(#[from] StoreError),
    #[error(transparent)]
    Host(#[from] renoa_local::LocalHostError),
    #[error("Telegram surface task failed: {0}")]
    Task(#[from] tokio::task::JoinError),
    #[error("Telegram surface stopped because {0}")]
    Supervision(String),
}
