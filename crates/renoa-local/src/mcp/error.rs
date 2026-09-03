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
    #[error("MCP Host background task failed: {0}")]
    Background(#[from] tokio::task::JoinError),
    #[error(transparent)]
    OAuth(#[from] McpOAuthError),
    #[error(transparent)]
    Adapter(#[from] McpAdapterError),
}

/// Failure while the Host owns an MCP OAuth lifecycle.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum McpOAuthError {
    #[error(
        "MCP connection '{0}' requires browser authorization; call extension_manage with action 'authorize'"
    )]
    AuthorizationRequired(String),
    #[error("MCP OAuth configuration is invalid: {0}")]
    Invalid(String),
    #[error("MCP OAuth for connection '{0}' is already running; wait for that flow to finish")]
    InProgress(String),
    #[error(
        "MCP OAuth outcome for connection '{connection}' is unknown; Renoa did not retry the credential exchange. Call extension_manage authorize with restart=true. Boundary error: {detail}"
    )]
    OutcomeUnknown { connection: String, detail: String },
    #[error(
        "MCP OAuth already completed for this recovered operation on connection '{0}', but its credential is no longer usable; start a new extension_manage authorize call"
    )]
    ReceiptUnavailable(String),
    #[error(
        "MCP OAuth previously returned a definite credential failure for this recovered operation on connection '{0}'; Renoa did not repeat the authorization flow. Start a new extension_manage authorize call to try again"
    )]
    ReceiptFailure(String),
    #[error("MCP OAuth callback expired; call extension_manage authorize with restart=true")]
    CallbackExpired,
    #[error("MCP OAuth callback was cancelled")]
    Cancelled,
    #[error("MCP OAuth authorization was rejected by the service: {0}")]
    CallbackRejected(String),
    #[error("MCP OAuth callback cannot resume: {0}")]
    CallbackUnavailable(String),
    #[error("could not open the MCP authorization page: {source}")]
    Browser {
        #[source]
        source: std::io::Error,
    },
    #[error("browser command did not open the MCP authorization page ({status})")]
    BrowserStatus { status: String },
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
    #[error("private Host credential storage failed: {0}")]
    PrivateStore(String),
    #[error("secure credential setup is unavailable: {0}")]
    SetupUnavailable(String),
    #[error("secure credential setup expired before a credential was received")]
    SetupExpired,
    #[error("secure credential setup returned invalid encrypted data")]
    SetupInvalid,
    #[error("{source_name} credential source could not start: {source}")]
    Start {
        source_name: &'static str,
        #[source]
        source: std::io::Error,
    },
    #[error("{0} credential source has no configured output pipe")]
    MissingPipe(&'static str),
    #[error("{source_name} credential source could not be waited: {source}")]
    Wait {
        source_name: &'static str,
        #[source]
        source: std::io::Error,
    },
    #[error("{source_name} credential source input could not be written: {source}")]
    Write {
        source_name: &'static str,
        #[source]
        source: std::io::Error,
    },
    #[error("{source_name} credential source cleanup failed: {detail}")]
    Cleanup {
        source_name: &'static str,
        detail: String,
    },
    #[error("{source_name} credential source {stream} reader failed: {source}")]
    Read {
        source_name: &'static str,
        stream: &'static str,
        #[source]
        source: std::io::Error,
    },
    #[error("{source_name} credential source {stream} reader task failed")]
    ReaderTask {
        source_name: &'static str,
        stream: &'static str,
        #[source]
        source: tokio::task::JoinError,
    },
    #[error("{0} credential lookup exceeded its deadline")]
    Timeout(&'static str),
    #[error("credential lookup was cancelled")]
    Cancelled,
    #[error("{0} credential output exceeded its boundary")]
    OutputLimit(&'static str),
    #[error("{0} returned an invalid credential token")]
    InvalidOutput(&'static str),
    #[error("{source_name} has no usable credential for {reference} ({status}); {guidance}")]
    Unavailable {
        source_name: &'static str,
        reference: String,
        status: String,
        guidance: String,
    },
}

impl From<McpCredentialError> for McpHostError {
    fn from(error: McpCredentialError) -> Self {
        Self::Adapter(McpAdapterError::Credential(error))
    }
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
    pub(super) fn redact_credential(&mut self, credential: Option<&super::McpCredentialHeader>) {
        let Some(credential) = credential else {
            return;
        };
        credential.redact_text(&mut self.message);
        if let Some(code) = &mut self.diagnostic.code {
            credential.redact_text(code);
        }
        if let Some(scope) = &mut self.diagnostic.required_scope {
            credential.redact_text(scope);
        }
        credential.redact_text(&mut self.diagnostic.detail);
    }

    pub(super) fn redact_exact_secrets<'a>(&mut self, secrets: impl IntoIterator<Item = &'a str>) {
        for secret in secrets {
            if secret.is_empty() {
                continue;
            }
            self.message = self.message.replace(secret, "[REDACTED]");
            if let Some(code) = &mut self.diagnostic.code {
                *code = code.replace(secret, "[REDACTED]");
            }
            if let Some(scope) = &mut self.diagnostic.required_scope {
                *scope = scope.replace(secret, "[REDACTED]");
            }
            self.diagnostic.detail = self.diagnostic.detail.replace(secret, "[REDACTED]");
        }
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
        if let Some(scope) = &self.diagnostic.required_scope {
            super::validate_oauth_scope(scope)
                .map_err(|error| format!("invalid required OAuth scope: {error}"))?;
        }
        if self
            .diagnostic
            .http_status
            .is_some_and(|status| !(100..=599).contains(&status))
        {
            return Err("failure HTTP status is outside 100-599".to_owned());
        }
        let insufficient_scope =
            self.diagnostic.code.as_deref() == Some("oauth_insufficient_scope");
        if self.diagnostic.required_scope.is_some() && !insufficient_scope {
            return Err(
                "required OAuth scope is only valid for oauth_insufficient_scope".to_owned(),
            );
        }
        if insufficient_scope
            && (self.kind != McpFailureKind::Protocol
                || self.certainty != McpOutcomeCertainty::Definite
                || self.partial_changes_possible
                || self.diagnostic.http_status != Some(403))
        {
            return Err(
                "oauth_insufficient_scope must be a definite HTTP 403 protocol failure".to_owned(),
            );
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

    #[must_use]
    pub fn required_oauth_scope(&self) -> Option<&str> {
        self.diagnostic.required_scope.as_deref()
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
struct McpFailureDiagnostic {
    #[serde(default)]
    code: Option<String>,
    #[serde(default)]
    http_status: Option<u16>,
    #[serde(default)]
    required_scope: Option<String>,
    detail: String,
}
