use std::path::PathBuf;

use serde::Deserialize;
use thiserror::Error;

#[derive(Debug, Error)]
#[non_exhaustive]
pub(crate) enum RegistryError {
    #[error("invalid official MCP Registry request: {0}")]
    Invalid(String),
    #[error("official MCP Registry discovery is unavailable: {0}")]
    Unavailable(String),
    #[error("cannot resolve official MCP Registry adapter: {0}")]
    Resolve(std::io::Error),
    #[error("official MCP Registry adapter is not a file: `{0}`")]
    NotFile(PathBuf),
    #[error("cannot start official MCP Registry adapter: {0}")]
    Start(std::io::Error),
    #[error("official MCP Registry adapter has no {0}")]
    MissingPipe(&'static str),
    #[error("cannot write official MCP Registry request: {0}")]
    Write(std::io::Error),
    #[error("cannot wait for official MCP Registry adapter: {0}")]
    Wait(std::io::Error),
    #[error("official MCP Registry adapter timed out")]
    Timeout,
    #[error("official MCP Registry request was cancelled")]
    Cancelled,
    #[error("official MCP Registry adapter output exceeded its bound")]
    OutputLimit,
    #[error("official MCP Registry adapter protocol failed: {0}")]
    Protocol(String),
    #[error("official MCP Registry process cleanup failed: {0}")]
    Cleanup(String),
    #[error("official MCP Registry request could not be encoded: {0}")]
    Encode(serde_json::Error),
    #[error("official MCP Registry rejected the request: {0}")]
    Remote(RegistryFailure),
    #[error("official MCP Registry output reader failed: {0}")]
    Reader(String),
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct RegistryFailure {
    kind: RegistryFailureKind,
    message: String,
    diagnostic: RegistryDiagnostic,
}

impl RegistryFailure {
    pub(crate) const fn kind(&self) -> RegistryFailureKind {
        self.kind
    }

    pub(crate) fn message(&self) -> &str {
        &self.message
    }

    pub(crate) const fn diagnostic(&self) -> &RegistryDiagnostic {
        &self.diagnostic
    }

    pub(super) fn validate(&self) -> Result<(), RegistryError> {
        require_bytes("Registry failure message", &self.message, 1, 1_024)?;
        if self.diagnostic.code.as_ref().is_some_and(|code| {
            code.is_empty()
                || code.len() > 128
                || !code.bytes().all(|byte| {
                    byte.is_ascii_lowercase() || byte.is_ascii_digit() || b"_-.".contains(&byte)
                })
        }) {
            return Err(protocol("Registry diagnostic code is malformed"));
        }
        require_bytes(
            "Registry diagnostic detail",
            &self.diagnostic.detail,
            0,
            4 * 1_024,
        )
    }
}

impl std::fmt::Display for RegistryFailure {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.message)
    }
}

#[derive(Clone, Copy, Debug, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum RegistryFailureKind {
    InvalidRequest,
    NotFound,
    Unavailable,
    Protocol,
    ResourceLimit,
    Timeout,
    Cancelled,
    Internal,
}

impl RegistryFailureKind {
    pub(crate) const fn as_str(self) -> &'static str {
        match self {
            Self::InvalidRequest => "invalid_request",
            Self::NotFound => "not_found",
            Self::Unavailable => "unavailable",
            Self::Protocol => "protocol",
            Self::ResourceLimit => "resource_limit",
            Self::Timeout => "timeout",
            Self::Cancelled => "cancelled",
            Self::Internal => "internal",
        }
    }
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct RegistryDiagnostic {
    #[serde(default)]
    code: Option<String>,
    #[serde(default)]
    http_status: Option<u16>,
    detail: String,
}

impl RegistryDiagnostic {
    pub(crate) fn code(&self) -> Option<&str> {
        self.code.as_deref()
    }

    pub(crate) const fn http_status(&self) -> Option<u16> {
        self.http_status
    }

    pub(crate) fn detail(&self) -> &str {
        &self.detail
    }
}

fn require_bytes(
    field: &str,
    value: &str,
    minimum: usize,
    maximum: usize,
) -> Result<(), RegistryError> {
    if (minimum..=maximum).contains(&value.len()) {
        Ok(())
    } else {
        Err(protocol(format!(
            "{field} must contain {minimum}-{maximum} UTF-8 bytes"
        )))
    }
}

fn protocol(message: impl Into<String>) -> RegistryError {
    RegistryError::Protocol(message.into())
}
