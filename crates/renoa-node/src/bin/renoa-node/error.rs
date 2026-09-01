use std::{io, path::PathBuf};

use renoa_local::LocalHostError;
use renoa_node::NodeError;
use thiserror::Error;
use tokio_tungstenite::tungstenite::Error as WebSocketError;

#[derive(Debug, Error)]
pub(crate) enum ServiceError {
    #[error("invalid node service configuration: {0}")]
    Configuration(String),
    #[error("failed to {action} `{path}`: {source}")]
    File {
        action: &'static str,
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    #[error("invalid JSON in `{path}`: {source}")]
    Json {
        path: PathBuf,
        #[source]
        source: serde_json::Error,
    },
    #[error("RCP message serialization failed: {0}")]
    Serialization(#[source] serde_json::Error),
    #[error("local Host setup failed: {0}")]
    Host(#[from] LocalHostError),
    #[error(transparent)]
    Node(#[from] NodeError),
    #[error("RCP enrollment transport failed: {0}")]
    EnrollmentTransport(#[from] WebSocketError),
    #[error("RCP enrollment protocol failed: {0}")]
    EnrollmentProtocol(String),
    #[error(
        "the coordinator issued a device credential, but it could not be saved to `{path}`: \
         {source}; create a new enrollment token before retrying"
    )]
    IssuedCredentialNotSaved {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    #[error(
        "the coordinator issued a device credential, but it could not be encoded for `{path}`: \
         {source}; create a new enrollment token before retrying"
    )]
    IssuedCredentialNotEncoded {
        path: PathBuf,
        #[source]
        source: serde_json::Error,
    },
    #[error("failed to install the process shutdown handler: {0}")]
    Signal(#[source] io::Error),
}

impl ServiceError {
    pub(crate) fn file(action: &'static str, path: &std::path::Path, source: io::Error) -> Self {
        Self::File {
            action,
            path: path.to_path_buf(),
            source,
        }
    }
}
