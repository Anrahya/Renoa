use std::io;

use renoa_local::{LocalHostError, LocalSessionError};
use thiserror::Error;

#[derive(Debug, Error)]
#[non_exhaustive]
pub enum ServerError {
    #[error("invalid ACP request: {0}")]
    InvalidRequest(String),
    #[error("invalid Renoa configuration: {0}")]
    Configuration(String),
    #[error("ACP session storage failed: {0}")]
    Io(#[from] io::Error),
    #[error(transparent)]
    Host(#[from] LocalHostError),
    #[error("ACP transport failed: {0}")]
    Transport(#[source] agent_client_protocol::Error),
    #[error("Renoa operation failed: {0}")]
    Operation(String),
}

impl ServerError {
    pub(crate) fn into_protocol_error(self) -> agent_client_protocol::Error {
        match self {
            Self::InvalidRequest(message)
            | Self::Host(LocalHostError::InvalidRequest(message)) => {
                agent_client_protocol::Error::invalid_params().data(message)
            }
            Self::Host(LocalHostError::Session(
                error @ LocalSessionError::UnfinishedOperation { .. },
            )) => {
                agent_client_protocol::Error::invalid_params().data(error.to_string())
            }
            Self::Host(LocalHostError::Session(LocalSessionError::CommandConflict {
                command_id,
                operation_id,
            })) => agent_client_protocol::Error::invalid_params().data(format!(
                "prompt identity {command_id} is already bound to operation {operation_id} with different content"
            )),
            error => agent_client_protocol::Error::internal_error().data(error.to_string()),
        }
    }
}
