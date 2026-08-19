//! Durable execution laws for composable Renoa agent runtimes.

mod admission;
mod cancellation;
mod database;
mod decision_store;
mod drive;
mod effect_store;
mod effect_supervision;
mod events;
mod ids;
mod inspection;
mod operation_phase;
mod runtime;
mod schema;
mod state;
mod unknown_effect;

use std::{
    collections::HashMap,
    error::Error as StdError,
    fmt,
    path::Path,
    sync::{Arc, Mutex},
};

pub use cancellation::{
    CancellationEffect, CancellationInput, CancellationTransition, UnsettledEffect,
};
use database::DatabaseLease;
pub use events::{EventCursor, EventPage, SemanticEvent};
pub use ids::{AgentId, CancellationId, CommandId, EffectId, EventId, OperationId, SessionId};
pub use runtime::{
    Checkpoint, EffectAdapter, EffectBinding, EffectCompletion, EffectFuture, EffectInvocation,
    EffectOutcome, EffectRecovery, LoopBinding, LoopDecision, LoopError, LoopInput, LoopPlugin,
    NewEvent, Runtime, RuntimeError, RuntimeManifest, SettledEffect, UnknownEffect,
    UnknownEffectAbandonment, UnknownEffectInput,
};
pub use state::{
    Admission, Command, DriveResult, EffectSnapshot, EffectStatus, OperationOutcome,
    OperationSnapshot, OperationStatus, SessionSnapshot,
};
use thiserror::Error;

/// A caller-actionable class of kernel storage failure.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum StoreErrorKind {
    Io,
    Sqlite,
    Ownership,
    Identity,
    UnsupportedPlatform,
}

#[derive(Debug)]
enum StoreSource {
    Io(std::io::Error),
    Sqlite(rusqlite::Error),
}

/// A storage failure with a stable classification and preserved source.
#[derive(Debug)]
pub struct StoreError {
    kind: StoreErrorKind,
    context: String,
    source: Option<StoreSource>,
}

impl StoreError {
    #[must_use]
    pub const fn kind(&self) -> StoreErrorKind {
        self.kind
    }

    pub(crate) fn io(action: &str, path: &Path, source: std::io::Error) -> Self {
        Self {
            kind: StoreErrorKind::Io,
            context: format!("{action} {}", path.display()),
            source: Some(StoreSource::Io(source)),
        }
    }

    pub(crate) fn sqlite(source: rusqlite::Error) -> Self {
        Self {
            kind: StoreErrorKind::Sqlite,
            context: "SQLite operation failed".to_owned(),
            source: Some(StoreSource::Sqlite(source)),
        }
    }

    pub(crate) fn message(kind: StoreErrorKind, context: impl Into<String>) -> Self {
        Self {
            kind,
            context: context.into(),
            source: None,
        }
    }
}

impl fmt::Display for StoreError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.context)?;
        match &self.source {
            Some(StoreSource::Io(source)) => write!(formatter, ": {source}"),
            Some(StoreSource::Sqlite(source)) => write!(formatter, ": {source}"),
            None => Ok(()),
        }
    }
}

impl StdError for StoreError {
    fn source(&self) -> Option<&(dyn StdError + 'static)> {
        match &self.source {
            Some(StoreSource::Io(source)) => Some(source),
            Some(StoreSource::Sqlite(source)) => Some(source),
            None => None,
        }
    }
}

/// A failure that prevented the kernel from preserving its invariants.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum KernelError {
    #[error("a kernel already owns {path}")]
    AlreadyRunning { path: std::path::PathBuf },
    #[error("kernel database aliases are unsupported: {path}")]
    UnsupportedDatabaseAlias { path: std::path::PathBuf },
    #[error("agent {0} was not found")]
    AgentNotFound(AgentId),
    #[error("session {0} was not found")]
    SessionNotFound(SessionId),
    #[error("session {session_id} is already bound to agent {agent_id}")]
    SessionConflict {
        session_id: SessionId,
        agent_id: AgentId,
    },
    #[error("command {command_id} is already bound to operation {operation_id}")]
    CommandConflict {
        command_id: CommandId,
        operation_id: OperationId,
    },
    #[error("operation {0} has no unknown effect to abandon")]
    NoUnknownEffect(OperationId),
    #[error("cancellation {cancellation_id} is already bound to operation {operation_id}")]
    CancellationConflict {
        cancellation_id: CancellationId,
        operation_id: OperationId,
    },
    #[error("operation {0} is not active and cancellable")]
    OperationNotCancellable(OperationId),
    #[error("operation {0} has a committed cancellation request")]
    CancellationPending(OperationId),
    #[error("session {0} already has an active driver")]
    Busy(SessionId),
    #[error("kernel effect execution requires a Tokio runtime")]
    RuntimeUnavailable,
    #[error("effect supervisor task failed: {0}")]
    EffectTask(#[source] tokio::task::JoinError),
    #[error("the supplied runtime does not match the operation's frozen manifest")]
    RuntimeMismatch,
    #[error("loop checkpoint schema {found} does not match frozen schema {expected}")]
    CheckpointSchemaMismatch { expected: u32, found: u32 },
    #[error("loop decision is invalid: {0}")]
    InvalidDecision(String),
    #[error("loop plugin failed: {0}")]
    Loop(LoopError),
    #[error("effect binding `{0}` is unavailable")]
    EffectBindingUnavailable(String),
    #[error("event cursor {cursor} is ahead of high-water mark {high_water}")]
    CursorAhead { cursor: u64, high_water: u64 },
    #[error("kernel schema version {found} is newer than supported version {supported}")]
    UnsupportedSchema { found: u32, supported: u32 },
    #[error("operation state version {found} is newer than supported version {supported}")]
    UnsupportedStateVersion { found: u32, supported: u32 },
    #[error("kernel data is invalid: {0}")]
    Corrupt(String),
    #[error("kernel storage failed: {0}")]
    Store(#[source] StoreError),
}

/// The exclusive durable owner of one Renoa kernel database.
pub struct Kernel {
    database: Arc<DatabaseLease>,
    running_sessions: effect_supervision::RunningSessions,
    #[cfg(test)]
    crash_point: Option<CrashPoint>,
}

impl Kernel {
    /// Opens or creates a kernel database and acquires its lifetime writer lock.
    ///
    /// # Errors
    ///
    /// Returns a typed ownership, compatibility, or storage failure.
    pub fn open(path: impl AsRef<Path>) -> Result<Self, KernelError> {
        let database = Arc::new(DatabaseLease::acquire(path.as_ref())?);
        let mut connection = database.connection()?;
        schema::initialize(&mut connection)?;
        Ok(Self {
            database,
            running_sessions: Arc::new(Mutex::new(HashMap::new())),
            #[cfg(test)]
            crash_point: None,
        })
    }

    #[cfg(test)]
    pub(crate) fn crash_at(&mut self, crash_point: CrashPoint) {
        self.crash_point = Some(crash_point);
    }

    #[cfg(test)]
    pub(crate) fn crash_if(&self, reached: CrashPoint) {
        assert_ne!(
            self.crash_point,
            Some(reached),
            "injected crash at {reached:?}"
        );
    }
}

#[cfg(test)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CrashPoint {
    ActivationCommitted,
    EffectIntentCommitted,
    EffectDispatchCommitted,
    EffectCompletedBeforeSettlement,
    EffectSettlementCommitted,
    UnknownEffectAbandonmentCommitted,
    CancellationCommitted,
    TerminalCommitted,
}

#[cfg(test)]
mod tests;
