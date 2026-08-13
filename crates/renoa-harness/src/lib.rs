//! Durable ownership and recovery for Renoa agent sessions.

use std::{
    collections::{HashMap, HashSet},
    num::NonZeroU32,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
};

use renoa_agent::{Model, Tool};
use thiserror::Error;
use tokio_util::sync::CancellationToken;

mod activation_store;
mod cancellation_store;
mod database;
mod drive;
mod recovery_store;
mod schema;
mod settlement_store;
mod state;
mod store;
mod store_support;
mod tool_cancellation_store;
mod tool_recovery_store;
mod tool_resolution_store;
mod tool_store;

use database::DatabaseLease;
pub use state::{
    Admission, CancellationId, OperationId, OperationOutcome, OperationRequest, OperationSnapshot,
    OperationStatus, OutputId, OutputRecord, RequestId, RunNext, SessionId, SessionSnapshot,
    ToolRecovery,
};
use state::{FrozenRuntime, FrozenTool};
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
    #[error("tool `{name}` is unavailable from runtime profile `{revision}`")]
    ToolBindingUnavailable { name: String, revision: String },
    #[error("operation {0} has no unresolved tool outcome")]
    NoUnknownToolOutcome(OperationId),
    #[error("cancellation {cancellation_id} is already bound to operation {operation_id}")]
    CancellationConflict {
        cancellation_id: CancellationId,
        operation_id: OperationId,
    },
    #[error("operation {0} is not active and cancellable")]
    OperationNotCancellable(OperationId),
}

/// The exclusive cooperating owner of one database in a trusted directory.
pub struct Harness {
    store: Store,
    running_sessions: Arc<Mutex<HashMap<SessionId, RunningSession>>>,
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
            running_sessions: Arc::new(Mutex::new(HashMap::new())),
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

    /// Fails one operation whose unsafe tool outcome cannot be recovered,
    /// while committing error results for every unresolved call in its batch.
    /// Exact retries return the already committed outcome.
    ///
    /// # Errors
    ///
    /// Returns [`HarnessError::NoUnknownToolOutcome`] when the target is not
    /// paused on an unknown tool effect.
    pub async fn abandon_unknown_tool(
        &self,
        session_id: SessionId,
        operation_id: OperationId,
    ) -> Result<OperationOutcome, HarnessError> {
        let lease = self.begin_run(session_id)?;
        self.store
            .abandon_unknown_tool(&lease, session_id, operation_id)
            .await
    }

    /// Durably requests cancellation of the active standalone operation.
    /// Exact retries with the same cancellation identity are idempotent.
    ///
    /// # Errors
    ///
    /// Returns a conflict when the cancellation identity was previously bound
    /// to another operation, or [`HarnessError::OperationNotCancellable`] when
    /// the target is not the session's active runnable operation.
    pub async fn request_standalone_cancellation(
        &self,
        session_id: SessionId,
        operation_id: OperationId,
        cancellation_id: CancellationId,
    ) -> Result<(), HarnessError> {
        let running = Arc::clone(&self.running_sessions);
        self.store
            .request_cancellation(session_id, operation_id, cancellation_id, move || {
                let cancellation = running
                    .lock()
                    .map_err(|_| {
                        HarnessError::Store("session ownership lock was poisoned".to_owned())
                    })?
                    .get(&session_id)
                    .filter(|active| active.operation_id == Some(operation_id))
                    .map(|active| active.cancellation.clone());
                if let Some(cancellation) = cancellation {
                    cancellation.cancel();
                }
                Ok(())
            })
            .await
    }

    fn begin_run(&self, session_id: SessionId) -> Result<Arc<SessionRunLease>, HarnessError> {
        let mut running = self
            .running_sessions
            .lock()
            .map_err(|_| HarnessError::Store("session ownership lock was poisoned".to_owned()))?;
        if running.contains_key(&session_id) {
            return Err(HarnessError::Busy(session_id));
        }
        let cancellation = CancellationToken::new();
        running.insert(
            session_id,
            RunningSession {
                operation_id: None,
                cancellation: cancellation.clone(),
            },
        );
        Ok(Arc::new(SessionRunLease {
            running: Arc::clone(&self.running_sessions),
            session_id,
            cancellation,
        }))
    }
}

#[cfg(test)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CrashPoint {
    ActivationCommitted,
    ModelIntentCommitted,
    ModelCompletedBeforeSettlement,
    ToolPlanCommitted,
    ToolIntentCommitted,
    ToolCompletedBeforeSettlement,
    ToolSettlementCommitted,
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
    tools: Vec<ToolBinding>,
    max_tool_calls_per_step: u32,
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
            tools: Vec::new(),
            max_tool_calls_per_step: 0,
        }
    }

    /// Installs the tools available to this profile and freezes an explicit
    /// per-response tool-call limit.
    ///
    /// # Errors
    ///
    /// Returns [`RuntimeProfileError::DuplicateToolName`] when two bindings
    /// advertise the same model-visible name.
    pub fn with_tools(
        mut self,
        tools: Vec<ToolBinding>,
        max_tool_calls_per_step: NonZeroU32,
    ) -> Result<Self, RuntimeProfileError> {
        let mut names = HashSet::with_capacity(tools.len());
        for binding in &tools {
            let name = binding.tool.spec().name.as_str();
            if binding.binding_id.is_empty() {
                return Err(RuntimeProfileError::EmptyToolBindingId(name.to_owned()));
            }
            if !names.insert(name) {
                return Err(RuntimeProfileError::DuplicateToolName(name.to_owned()));
            }
        }
        self.tools = tools;
        self.max_tool_calls_per_step = max_tool_calls_per_step.get();
        Ok(self)
    }

    fn frozen(&self) -> FrozenRuntime {
        FrozenRuntime {
            revision: self.revision.clone(),
            system_prompt: self.system_prompt.clone(),
            max_model_attempts: self.max_model_attempts.get(),
            max_tool_calls_per_step: self.max_tool_calls_per_step,
            tools: self
                .tools
                .iter()
                .map(|binding| FrozenTool {
                    binding_id: Some(binding.binding_id.clone()),
                    spec: binding.tool.spec().clone(),
                    recovery: binding.recovery,
                })
                .collect(),
        }
    }

    fn resolve_tool(&self, frozen: &FrozenTool) -> Result<Arc<dyn Tool>, HarnessError> {
        self.tools
            .iter()
            .find(|binding| {
                frozen.binding_id.as_deref() == Some(binding.binding_id.as_str())
                    && binding.tool.spec() == &frozen.spec
                    && binding.recovery == frozen.recovery
            })
            .map(|binding| Arc::clone(&binding.tool))
            .ok_or_else(|| HarnessError::ToolBindingUnavailable {
                name: frozen.spec.name.clone(),
                revision: self.revision.clone(),
            })
    }
}

/// One tool implementation plus its crash-recovery declaration.
pub struct ToolBinding {
    binding_id: String,
    tool: Arc<dyn Tool>,
    recovery: ToolRecovery,
}

impl ToolBinding {
    /// Binds one tool implementation to a stable host-managed identity.
    /// Change `binding_id` whenever behavior that matters to recovery changes.
    #[must_use]
    pub fn new(binding_id: impl Into<String>, tool: Arc<dyn Tool>, recovery: ToolRecovery) -> Self {
        Self {
            binding_id: binding_id.into(),
            tool,
            recovery,
        }
    }
}

#[derive(Debug, Error, PartialEq, Eq)]
#[non_exhaustive]
pub enum RuntimeProfileError {
    #[error("tool name `{0}` is configured more than once")]
    DuplicateToolName(String),
    #[error("tool `{0}` has an empty binding identity")]
    EmptyToolBindingId(String),
}

pub(crate) struct SessionRunLease {
    running: Arc<Mutex<HashMap<SessionId, RunningSession>>>,
    session_id: SessionId,
    cancellation: CancellationToken,
}

impl SessionRunLease {
    pub(crate) fn cancellation(&self) -> CancellationToken {
        self.cancellation.clone()
    }

    pub(crate) fn bind_operation(&self, operation_id: OperationId) -> Result<(), HarnessError> {
        let mut running = self
            .running
            .lock()
            .map_err(|_| HarnessError::Store("session ownership lock was poisoned".to_owned()))?;
        let session = running.get_mut(&self.session_id).ok_or_else(|| {
            HarnessError::Store("session ownership disappeared while running".to_owned())
        })?;
        session.operation_id = Some(operation_id);
        Ok(())
    }
}

struct RunningSession {
    operation_id: Option<OperationId>,
    cancellation: CancellationToken,
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
