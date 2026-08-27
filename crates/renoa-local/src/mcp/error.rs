use std::path::PathBuf;

use serde::Deserialize;
use thiserror::Error;

const MAX_FAILURE_MESSAGE_BYTES: usize = 512;
const MAX_DIAGNOSTIC_CODE_BYTES: usize = 128;
const MAX_DIAGNOSTIC_BYTES: usize = 4 * 1_024;

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
    #[error(transparent)]
    HostCatalog(#[from] crate::host::catalog::HostCatalogError),
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
    #[error("MCP adapter request exceeded the process boundary")]
    InputLimit,
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
    #[error("MCP adapter invocation was cancelled")]
    Cancelled,
    #[error("MCP adapter output exceeded the process boundary")]
    OutputLimit,
    #[error("MCP adapter returned an invalid record: {0}")]
    Protocol(String),
    #[error(transparent)]
    Credential(#[from] McpCredentialError),
    #[error("MCP adapter reported a remote failure: {0}")]
    Remote(McpRemoteFailure),
}

/// Failure while resolving a short-lived MCP credential from a local source.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum McpCredentialError {
    #[error("GitHub CLI credential source could not start: {0}")]
    Start(#[source] std::io::Error),
    #[error("GitHub CLI credential source has no configured output pipe")]
    MissingPipe,
    #[error("GitHub CLI credential source could not be waited: {0}")]
    Wait(#[source] std::io::Error),
    #[error("GitHub CLI credential source cleanup failed: {0}")]
    Cleanup(String),
    #[error("GitHub CLI credential source {stream} reader failed: {source}")]
    Read {
        stream: &'static str,
        #[source]
        source: std::io::Error,
    },
    #[error("GitHub CLI credential source {0} reader task failed")]
    ReaderTask(&'static str, #[source] tokio::task::JoinError),
    #[error("GitHub CLI credential lookup exceeded its deadline")]
    Timeout,
    #[error("GitHub CLI credential lookup was cancelled")]
    Cancelled,
    #[error("GitHub CLI credential output exceeded its boundary")]
    OutputLimit,
    #[error("GitHub CLI returned an invalid credential token")]
    InvalidOutput,
    #[error(
        "GitHub CLI has no usable token for account `{account}` on `{hostname}` ({status}); run `gh auth status --hostname {hostname}`"
    )]
    Unavailable {
        hostname: String,
        account: String,
        status: String,
    },
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
    pub(super) fn redact_authorization(&mut self, authorization: Option<&super::McpAuthorization>) {
        let Some(authorization) = authorization else {
            return;
        };
        authorization.redact_text(&mut self.message);
        if let Some(code) = &mut self.diagnostic.code {
            authorization.redact_text(code);
        }
        authorization.redact_text(&mut self.diagnostic.detail);
    }

    pub(super) fn validate_wire(&self) -> Result<(), String> {
        if self.message.is_empty() || self.message.len() > MAX_FAILURE_MESSAGE_BYTES {
            return Err(format!(
                "failure message must be 1-{MAX_FAILURE_MESSAGE_BYTES} bytes"
            ));
        }
        if self.certainty == McpOutcomeCertainty::Unknown && !self.partial_changes_possible {
            return Err("unknown failure must allow partial remote changes".to_owned());
        }
        if self
            .diagnostic
            .code
            .as_ref()
            .is_some_and(|code| code.is_empty() || code.len() > MAX_DIAGNOSTIC_CODE_BYTES)
        {
            return Err(format!(
                "failure diagnostic code must be 1-{MAX_DIAGNOSTIC_CODE_BYTES} bytes"
            ));
        }
        if self.diagnostic.detail.len() > MAX_DIAGNOSTIC_BYTES {
            return Err(format!(
                "failure diagnostic exceeds {MAX_DIAGNOSTIC_BYTES} bytes"
            ));
        }
        if self
            .diagnostic
            .http_status
            .is_some_and(|status| !(100..=599).contains(&status))
        {
            return Err("failure HTTP status is outside 100-599".to_owned());
        }
        Ok(())
    }

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
