use std::path::Path;

use uuid::Uuid;

use super::{LocalHost, LocalHostError, sessions::StoredSession};
use crate::{
    LocalHistoryEntry, LocalSession,
    trace::{TRACE_DATABASE, TraceStore},
};

/// An existing session held for history inspection without an execution runtime.
///
/// This handle retains exclusive kernel ownership until dropped. It cannot
/// execute, recover a turn, or change the saved runtime selection.
pub struct AgentSessionHistory {
    id: Uuid,
    kernel: LocalSession,
    diagnostic_error: Option<String>,
}

impl AgentSessionHistory {
    #[must_use]
    pub const fn id(&self) -> Uuid {
        self.id
    }

    /// Returns the separate diagnostic-store problem, if any.
    #[must_use]
    pub fn diagnostic_error(&self) -> Option<&str> {
        self.diagnostic_error.as_deref()
    }

    /// Projects the complete durable transcript with its original event identities.
    ///
    /// # Errors
    ///
    /// Returns authoritative storage or history corruption errors.
    pub fn history(&self) -> Result<Vec<LocalHistoryEntry>, LocalHostError> {
        Ok(self.kernel.history()?)
    }
}

impl LocalHost {
    /// Opens existing history without discovering models or resolving a runtime.
    ///
    /// The registered profile, exact session/agent identity, workspace binding,
    /// and exclusive kernel ownership are checked just as for executable loading.
    /// Diagnostic failures are reported by the handle and do not hide history.
    /// Drop the handle before loading the session for execution.
    ///
    /// # Errors
    ///
    /// Returns identity, workspace binding, ownership, or authoritative storage
    /// failures. Corrupt kernel history never becomes a successful inspection.
    pub async fn inspect_session(
        &self,
        session_uuid: Uuid,
        cwd: &Path,
    ) -> Result<AgentSessionHistory, LocalHostError> {
        let StoredSession {
            directory,
            manifest,
            kernel,
        } = self.load_session_storage(session_uuid, cwd).await?;
        kernel.history()?;
        let diagnostic_error = TraceStore::open(
            directory.join(TRACE_DATABASE),
            manifest.session_id,
            manifest.agent_id,
            &manifest.profile,
        )
        .err()
        .map(|error| error.to_string());
        Ok(AgentSessionHistory {
            id: session_uuid,
            kernel,
            diagnostic_error,
        })
    }
}
