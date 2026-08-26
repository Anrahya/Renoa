use std::path::PathBuf;

use serde::Deserialize;
use thiserror::Error;

/// Failure while configuring, discovering, or loading Host-owned MCP pieces.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum McpHostError {
    #[error("invalid MCP configuration: {0}")]
    Invalid(String),
    #[error("MCP configuration conflicts with durable state: {0}")]
    Conflict(String),
    #[error("MCP configuration is missing: {0}")]
    NotFound(String),
    #[error("MCP Host storage failed: {0}")]
    Io(#[from] std::io::Error),
    #[error("MCP Host catalog failed: {0}")]
    Database(#[from] rusqlite::Error),
    #[error("MCP catalog JSON failed: {0}")]
    Json(#[from] serde_json::Error),
    #[error(transparent)]
    Adapter(#[from] McpAdapterError),
}

/// Failure at the replaceable MCP process boundary.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum McpAdapterError {
    #[error("MCP adapter cannot be resolved: {0}")]
    Resolve(#[source] std::io::Error),
    #[error("MCP adapter is not a regular file: {0}")]
    NotFile(PathBuf),
    #[error("MCP adapter request could not be encoded: {0}")]
    Encode(#[source] serde_json::Error),
    #[error("MCP adapter could not start: {0}")]
    Start(#[source] std::io::Error),
    #[error("MCP adapter process has no {0}")]
    MissingPipe(&'static str),
    #[error("MCP adapter request could not be written: {0}")]
    Write(#[source] std::io::Error),
    #[error("MCP adapter process could not be waited: {0}")]
    Wait(#[source] std::io::Error),
    #[error("MCP adapter process cleanup failed: {0}")]
    Cleanup(String),
    #[error("MCP adapter {stream} reader failed: {source}")]
    Read {
        stream: &'static str,
        #[source]
        source: std::io::Error,
    },
    #[error("MCP adapter {0} reader task failed")]
    ReaderTask(&'static str, #[source] tokio::task::JoinError),
    #[error("MCP adapter exceeded its Host deadline")]
    Timeout,
    #[error("MCP adapter output exceeded the process boundary")]
    OutputLimit,
    #[error("MCP adapter returned an invalid record: {0}")]
    Protocol(String),
    #[error("MCP discovery failed: {0}")]
    Remote(McpRemoteFailure),
}

/// Adapter failure class from the pinned MCP process wire.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum McpFailureKind {
    InvalidRequest,
    InvalidEndpoint,
    IncompatibleProtocol,
    Protocol,
    ResourceLimit,
    Timeout,
    Cancelled,
    Unavailable,
    UnsupportedResult,
    InvalidResult,
    Transport,
    Internal,
}

impl McpFailureKind {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::InvalidRequest => "invalid_request",
            Self::InvalidEndpoint => "invalid_endpoint",
            Self::IncompatibleProtocol => "incompatible_protocol",
            Self::Protocol => "protocol",
            Self::ResourceLimit => "resource_limit",
            Self::Timeout => "timeout",
            Self::Cancelled => "cancelled",
            Self::Unavailable => "unavailable",
            Self::UnsupportedResult => "unsupported_result",
            Self::InvalidResult => "invalid_result",
            Self::Transport => "transport",
            Self::Internal => "internal",
        }
    }
}

impl std::fmt::Display for McpFailureKind {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(self.as_str())
    }
}

/// Whether the adapter can prove the remote outcome.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum McpOutcomeCertainty {
    Definite,
    Unknown,
}

impl McpOutcomeCertainty {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Definite => "definite",
            Self::Unknown => "unknown",
        }
    }
}

/// Typed failure reported by the MCP process adapter.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct McpRemoteFailure {
    kind: McpFailureKind,
    certainty: McpOutcomeCertainty,
    message: String,
    partial_changes_possible: bool,
    diagnostic: McpFailureDiagnostic,
}

impl std::fmt::Display for McpRemoteFailure {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "{}: {}", self.kind, self.message)
    }
}

impl McpRemoteFailure {
    #[must_use]
    pub const fn kind(&self) -> McpFailureKind {
        self.kind
    }

    #[must_use]
    pub const fn certainty(&self) -> McpOutcomeCertainty {
        self.certainty
    }

    #[must_use]
    pub fn message(&self) -> &str {
        &self.message
    }

    #[must_use]
    pub const fn partial_changes_possible(&self) -> bool {
        self.partial_changes_possible
    }

    #[must_use]
    pub fn diagnostic_code(&self) -> Option<&str> {
        self.diagnostic.code.as_deref()
    }

    #[must_use]
    pub const fn diagnostic_http_status(&self) -> Option<u16> {
        self.diagnostic.http_status
    }

    #[must_use]
    pub fn diagnostic_detail(&self) -> &str {
        &self.diagnostic.detail
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
struct McpFailureDiagnostic {
    #[serde(default)]
    code: Option<String>,
    #[serde(default)]
    http_status: Option<u16>,
    detail: String,
}
