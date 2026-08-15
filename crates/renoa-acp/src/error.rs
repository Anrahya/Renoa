use std::io;

use renoa_harness::HarnessError;
use renoa_local::{LocalRuntimeError, LocalWorkspaceError, PiModelConfigError};
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
    #[error("ACP session metadata is invalid: {0}")]
    Metadata(#[from] serde_json::Error),
    #[error(transparent)]
    Workspace(#[from] LocalWorkspaceError),
    #[error(transparent)]
    Runtime(#[from] LocalRuntimeError),
    #[error(transparent)]
    Model(#[from] PiModelConfigError),
    #[error(transparent)]
    Harness(#[from] HarnessError),
    #[error("ACP background storage task failed: {0}")]
    Background(#[from] tokio::task::JoinError),
    #[error("ACP transport failed: {0}")]
    Transport(#[source] agent_client_protocol::Error),
    #[error("Renoa operation failed: {0}")]
    Operation(String),
}

impl ServerError {
    pub(crate) fn into_protocol_error(self) -> agent_client_protocol::Error {
        match self {
            Self::InvalidRequest(message) => {
                agent_client_protocol::Error::invalid_params().data(message)
            }
            error => agent_client_protocol::Error::internal_error().data(error.to_string()),
        }
    }
}
