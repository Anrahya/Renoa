use std::path::PathBuf;

use serde::Deserialize;
use thiserror::Error;

#[derive(Debug, Error)]
#[non_exhaustive]
pub enum CatalogError {
    #[error("cannot resolve integration catalog adapter: {0}")]
    Resolve(std::io::Error),
    #[error("integration catalog adapter is not a file: `{0}`")]
    NotFile(PathBuf),
    #[error("cannot start integration catalog adapter: {0}")]
    Start(std::io::Error),
    #[error("integration catalog adapter has no {0}")]
    MissingPipe(&'static str),
    #[error("cannot write integration catalog request: {0}")]
    Write(std::io::Error),
    #[error("cannot wait for integration catalog adapter: {0}")]
    Wait(std::io::Error),
    #[error("integration catalog adapter timed out")]
    Timeout,
    #[error("integration catalog request was cancelled")]
    Cancelled,
    #[error("integration catalog adapter output exceeded its bound")]
    OutputLimit,
    #[error("integration catalog adapter protocol failed: {0}")]
    Protocol(String),
    #[error("integration catalog process cleanup failed: {0}")]
    Cleanup(String),
    #[error("integration catalog request could not be encoded: {0}")]
    Encode(serde_json::Error),
    #[error("integration catalog rejected the request: {0}")]
    Remote(CatalogFailure),
    #[error("integration catalog output reader failed: {0}")]
    Reader(String),
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CatalogFailure {
    kind: CatalogFailureKind,
    message: String,
    #[serde(default)]
    diagnostic: Option<CatalogDiagnostic>,
}

impl CatalogFailure {
    #[must_use]
    pub const fn kind(&self) -> CatalogFailureKind {
        self.kind
    }

    #[must_use]
    pub fn message(&self) -> &str {
        &self.message
    }

    #[must_use]
    pub const fn diagnostic(&self) -> Option<&CatalogDiagnostic> {
        self.diagnostic.as_ref()
    }

    pub(super) fn validate(&self) -> Result<(), CatalogError> {
        if self.message.is_empty() || self.message.len() > 8 * 1_024 {
            return Err(protocol(
                "catalog failure message must contain 1-8192 UTF-8 bytes",
            ));
        }
        if let Some(diagnostic) = &self.diagnostic {
            if diagnostic.code.as_ref().is_some_and(|code| {
                code.is_empty()
                    || code.len() > 128
                    || !code.bytes().all(|byte| {
                        byte.is_ascii_lowercase() || byte.is_ascii_digit() || b"_-.".contains(&byte)
                    })
            }) {
                return Err(protocol("catalog diagnostic code is malformed"));
            }
            if diagnostic
                .detail
                .as_ref()
                .is_some_and(|detail| detail.len() > 4 * 1_024)
            {
                return Err(protocol(
                    "catalog diagnostic detail exceeds 4096 UTF-8 bytes",
                ));
            }
        }
        Ok(())
    }
}

impl std::fmt::Display for CatalogFailure {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.message)
    }
}

#[derive(Clone, Copy, Debug, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CatalogFailureKind {
    InvalidRequest,
    NotFound,
    Conflict,
    Unavailable,
    Protocol,
    ResourceLimit,
    Internal,
}

impl CatalogFailureKind {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::InvalidRequest => "invalid_request",
            Self::NotFound => "not_found",
            Self::Conflict => "conflict",
            Self::Unavailable => "unavailable",
            Self::Protocol => "protocol",
            Self::ResourceLimit => "resource_limit",
            Self::Internal => "internal",
        }
    }
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CatalogDiagnostic {
    #[serde(default)]
    code: Option<String>,
    #[serde(default)]
    http_status: Option<u16>,
    #[serde(default)]
    detail: Option<String>,
}

impl CatalogDiagnostic {
    #[must_use]
    pub fn code(&self) -> Option<&str> {
        self.code.as_deref()
    }

    #[must_use]
    pub const fn http_status(&self) -> Option<u16> {
        self.http_status
    }

    #[must_use]
    pub fn detail(&self) -> Option<&str> {
        self.detail.as_deref()
    }
}

fn protocol(message: impl Into<String>) -> CatalogError {
    CatalogError::Protocol(message.into())
}
