//! Public types, errors, and tuning constants for supervised live apps.

use std::{fmt, io, time::Duration};

use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::{host::catalog::HostCatalogError, sites::SiteError};

pub(crate) const SCRATCH_DIR_NAME: &str = "scratch";
pub(crate) const LOG_FILE_NAME: &str = "app.log";

/// Idle window in minutes before reclaiming a running live app.
pub const IDLE_TIMEOUT_MINUTES: u64 = 30;

/// Stop an idle live app after [`IDLE_TIMEOUT_MINUTES`] without a gateway request.
/// Long enough to cover interactive review sessions and brief breaks without
/// shutting down mid-thought, while reclaiming idle dev servers and Python
/// runtimes automatically.
pub const IDLE_TIMEOUT: Duration = Duration::from_mins(IDLE_TIMEOUT_MINUTES);

/// How long initial startup or on-demand wake waits for HTTP readiness on
/// `127.0.0.1:$PORT` before reporting failure instead of retrying forever.
pub const STARTUP_TIMEOUT: Duration = Duration::from_secs(15);

pub(crate) const MEMORY_MAX: &str = "512M";
pub(crate) const MEMORY_SWAP_MAX: &str = "0";
pub(crate) const CPU_QUOTA: &str = "100%";
pub(crate) const TASKS_MAX: u32 = 128;
pub(crate) const APPS_SLICE: &str = "renoa-apps.slice";
pub(crate) const SLICE_MEMORY_MAX: &str = "2G";
pub(crate) const SLICE_TASKS_MAX: u32 = 1024;
pub(crate) const MAX_RUNNING_APPS: usize = 4;
pub(crate) const CRASH_BURST: u32 = 3;
pub(crate) const CRASH_INTERVAL_SECS: u64 = 15;
pub(crate) const LOG_TAIL_LINES: usize = 50;
pub(crate) const LOG_TAIL_BYTES: usize = 16 * 1024;
pub(crate) const MAX_LOG_FILE_BYTES: usize = 1024 * 1024;
pub(crate) const MAX_SCRATCH_BYTES: u64 = 256 * 1024 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AppState {
    Running,
    Stopped,
    Failed,
}

impl AppState {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Running => "running",
            Self::Stopped => "stopped",
            Self::Failed => "failed",
        }
    }
}

impl fmt::Display for AppState {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LiveApp {
    pub name: String,
    pub agent_id: String,
    pub source_folder: String,
    pub command: String,
    pub port: u16,
    pub state: AppState,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub failure_reason: Option<String>,
    pub updated_at_ms: i64,
    pub last_active_ms: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AppLogs {
    pub app: LiveApp,
    pub logs: String,
}

#[derive(Debug, Error)]
#[non_exhaustive]
pub enum AppError {
    #[error("No live app exists with this name.")]
    NotFound,
    #[error(
        "App name must be 1 to 63 lowercase letters, numbers, or hyphens, starting and ending with a letter or number."
    )]
    InvalidName,
    #[error("This app name already belongs to another person.")]
    PermissionDenied,
    #[error("App folder is invalid: {0}")]
    InvalidFolder(String),
    #[error("App command must be a non-empty shell command without NUL bytes.")]
    InvalidCommand,
    #[error("No local app ports remain available in 20000..=29999.")]
    NoAvailablePort,
    #[error("Live app supervisor is unavailable: {0}")]
    SupervisorUnavailable(String),
    #[error("Live app failed to start: {reason}")]
    StartupFailed { reason: String },
    #[error("Live app `{name}` failed: {reason}")]
    Failed {
        name: String,
        reason: String,
        logs: String,
    },
    #[error(transparent)]
    Catalog(#[from] HostCatalogError),
    #[error("live app storage failed: {0}")]
    Io(#[from] io::Error),
}

impl From<SiteError> for AppError {
    fn from(err: SiteError) -> Self {
        match err {
            SiteError::InvalidName => Self::InvalidName,
            SiteError::InvalidFolder(msg) => Self::InvalidFolder(msg),
            SiteError::PermissionDenied => Self::PermissionDenied,
            SiteError::Catalog(e) => Self::Catalog(e),
            SiteError::Io(e) => Self::Io(e),
            other => Self::InvalidFolder(other.to_string()),
        }
    }
}
