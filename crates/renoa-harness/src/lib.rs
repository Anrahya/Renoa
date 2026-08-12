//! Durable ownership and recovery for Renoa agent sessions.

use std::{
    collections::HashSet,
    num::NonZeroU32,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
};

use renoa_agent::Model;
use thiserror::Error;

mod activation_store;
mod database;
mod drive;
mod recovery_store;
mod schema;
mod settlement_store;
mod state;
mod store;
mod store_support;

use database::DatabaseLease;
pub use state::{
    Admission, OperationId, OperationOutcome, OperationRequest, OperationSnapshot, OperationStatus,
    OutputId, OutputRecord, RequestId, RunNext, SessionId, SessionSnapshot,
};
use store::Store;

#[derive(Debug, Error, PartialEq, Eq)]
#[non_exhaustive]
pub enum HarnessError {
    #[error("a harness already owns {path}")]
    AlreadyRunning { path: PathBuf },
    #[error("harness database aliases are unsupported: {path}")]
    UnsupportedDatabaseAlias { path: PathBuf },
    #[error("session {0} was not found")]
    SessionNotFound(SessionId),
    #[error(
        "request {request_id} is already bound to operation {operation_id} with different content"
    )]
    RequestConflict {
        request_id: RequestId,
        operation_id: OperationId,
    },
    #[error("harness data is invalid: {0}")]
    Corrupt(String),
    #[error("harness schema version {found} is newer than supported version {supported}")]
    UnsupportedSchema { found: u32, supported: u32 },
    #[error("harness storage failed: {0}")]
    Store(String),
    #[error("session {0} already has an active driver")]
    Busy(SessionId),
    #[error("operation requires runtime profile `{required}`, but `{provided}` was supplied")]
    RuntimeProfileUnavailable { required: String, provided: String },
}

/// The exclusive cooperating owner of one database in a trusted directory.
pub struct Harness {
    store: Store,
    running_sessions: Arc<Mutex<HashSet<SessionId>>>,
    #[cfg(test)]
    crash_point: Option<CrashPoint>,
}

impl Harness {
    /// Opens the database after acquiring its lifetime-exclusive writer lock.
    ///
    /// # Errors
    ///
    /// Returns [`HarnessError::AlreadyRunning`] when another process owns the
    /// database, or a storage error when it cannot be initialized.
    pub fn open(path: impl AsRef<Path>) -> Result<Self, HarnessError> {
        let store = Store::open(DatabaseLease::acquire(path.as_ref())?)?;
        Ok(Self {
            store,
            running_sessions: Arc::new(Mutex::new(HashSet::new())),
            #[cfg(test)]
            crash_point: None,
        })
    }

    /// Ensures that a standalone session with this stable identity exists.
    ///
    /// # Errors
    ///
    /// Returns a storage error when the session cannot be committed.
    pub async fn create_standalone_session(
        &self,
        session_id: SessionId,
    ) -> Result<(), HarnessError> {
        self.store.create_session(session_id).await
    }

    /// Durably admits one operation before returning its stable position.
    ///
    /// # Errors
    ///
    /// Returns a conflict when the request identity was reused with different
    /// content, or a storage error when admission cannot be committed.
    pub async fn admit_standalone(
        &self,
        session_id: SessionId,
        request: OperationRequest,
    ) -> Result<Admission, HarnessError> {
        self.store.admit(session_id, request).await
    }

    /// Reads the durable conversation and ordered operation states.
    ///
    /// # Errors
    ///
    /// Returns [`HarnessError::SessionNotFound`] or an invalid-storage error.
    pub async fn inspect(&self, session_id: SessionId) -> Result<SessionSnapshot, HarnessError> {
        self.store.inspect(session_id).await
    }

    /// Runs or recovers the session's active operation, or claims its next
    /// queued operation. Exactly one driver may own a session at a time.
    ///
    /// # Errors
    ///
    /// Returns a typed ownership, profile, or storage failure. Provider
    /// failures are durable operation outcomes rather than harness errors.
    pub async fn run_next(
        &self,
        session_id: SessionId,
        profile: &RuntimeProfile,
    ) -> Result<RunNext, HarnessError> {
        let lease = self.begin_run(session_id)?;
        drive::run_next(
            &self.store,
            &lease,
            session_id,
            profile,
            #[cfg(test)]
            self.crash_point,
        )
        .await
    }

    fn begin_run(&self, session_id: SessionId) -> Result<Arc<SessionRunLease>, HarnessError> {
        let mut running = self
            .running_sessions
            .lock()
            .map_err(|_| HarnessError::Store("session ownership lock was poisoned".to_owned()))?;
        if !running.insert(session_id) {
            return Err(HarnessError::Busy(session_id));
        }
        Ok(Arc::new(SessionRunLease {
            running: Arc::clone(&self.running_sessions),
            session_id,
        }))
    }
}

#[cfg(test)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CrashPoint {
    ActivationCommitted,
    ModelIntentCommitted,
    ModelCompletedBeforeSettlement,
    SettlementCommitted,
}

#[cfg(test)]
impl Harness {
    fn crash_at(&mut self, point: CrashPoint) {
        self.crash_point = Some(point);
    }
}

/// The host-resolved model binding and configuration offered to an operation.
pub struct RuntimeProfile {
    revision: String,
    model: Arc<dyn Model>,
    system_prompt: String,
    max_model_attempts: NonZeroU32,
}

impl RuntimeProfile {
    #[must_use]
    pub fn new(
        revision: impl Into<String>,
        model: Arc<dyn Model>,
        system_prompt: impl Into<String>,
        max_model_attempts: NonZeroU32,
    ) -> Self {
        Self {
            revision: revision.into(),
            model,
            system_prompt: system_prompt.into(),
            max_model_attempts,
        }
    }
}

pub(crate) struct SessionRunLease {
    running: Arc<Mutex<HashSet<SessionId>>>,
    session_id: SessionId,
}

impl Drop for SessionRunLease {
    fn drop(&mut self) {
        if let Ok(mut running) = self.running.lock() {
            running.remove(&self.session_id);
        }
    }
}

#[cfg(test)]
mod tests;

#[cfg(test)]
pub(crate) struct ModelAttemptDiagnostic {
    pub(crate) status: String,
    pub(crate) usage: Option<renoa_agent::TokenUsage>,
    pub(crate) has_request: bool,
    pub(crate) error: Option<String>,
}

#[cfg(test)]
pub(crate) fn inspect_model_attempts(
    harness: &Harness,
    session_id: SessionId,
) -> Vec<ModelAttemptDiagnostic> {
    harness
        .store
        .inspect_model_attempts(session_id)
        .expect("inspect model attempts")
}
