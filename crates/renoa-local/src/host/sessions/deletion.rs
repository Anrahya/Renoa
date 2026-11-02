//! Deleting a session, and the refusals a claim on a session must not come
//! before: a claim cancels a note pass, so a call refused afterwards would
//! cancel it for nothing.

use renoa_kernel::{AgentId, SessionId};
use uuid::Uuid;

use super::super::{LocalHost, LocalHostError, catalog};
use crate::{
    host_storage::{MANIFEST_FILE, delete_session_storage, read_manifest_file},
    plugins::host::memory::note_pass,
};

impl LocalHost {
    /// Permanently removes one closed Agent session owned by `agent_id`.
    ///
    /// A session bound to a different agent, or open in this process for
    /// anything but a note pass, is refused before anything changes. A note
    /// pass reviewing it is then cancelled and waited for, as for a turn. Its
    /// storage goes first, outside any catalog transaction; then one short
    /// transaction removes what the note pass kept of it. A stop between the
    /// two leaves only those rows, which a retry removes: deleting a missing
    /// session succeeds, so a retried ACP request is safe.
    ///
    /// # Errors
    ///
    /// Returns a foreign-agent rejection, or an ownership, identity, metadata,
    /// or storage failure. A session another process owns cannot be deleted.
    pub async fn delete_session(
        &self,
        agent_id: AgentId,
        session_uuid: Uuid,
    ) -> Result<(), LocalHostError> {
        self.refuse_before_claim(agent_id, session_uuid).await?;
        let _claim = self.config.claims.surface_uncancelled(session_uuid).await?;
        let sessions = self.config.sessions.clone();
        let database = self.config.database.clone();
        let session_id = SessionId::from_uuid(session_uuid);
        tokio::task::spawn_blocking(move || {
            delete_session_storage(&sessions, agent_id, session_id)?;
            let now_ms = note_pass::now_ms(&database);
            let mut catalog = catalog::open_verified(&database)?;
            let transaction = catalog
                .transaction()
                .map_err(catalog::HostCatalogError::from)?;
            note_pass::store::forget(&transaction, session_uuid, now_ms)?;
            transaction
                .commit()
                .map_err(catalog::HostCatalogError::from)?;
            Ok::<_, LocalHostError>(())
        })
        .await??;
        self.config.claims.forget(session_uuid);
        let skills = self.config.skill_store.clone();
        tokio::task::spawn_blocking(move || skills.remove_session(session_id)).await??;
        Ok(())
    }

    /// Refuses a stored session of another agent, or one an instance in this
    /// process holds for anything but a note pass, which the kernel would
    /// refuse a second handle on, before a claim on it cancels a note pass.
    /// A missing session is left to the caller. The opening that follows
    /// checks again: another process's hold is found only there.
    ///
    /// # Errors
    ///
    /// Returns a foreign-agent or open-session rejection, or a metadata
    /// failure.
    pub(in crate::host) async fn refuse_before_claim(
        &self,
        agent_id: AgentId,
        session_uuid: Uuid,
    ) -> Result<(), LocalHostError> {
        let manifest = self
            .config
            .sessions
            .join(SessionId::from_uuid(session_uuid).to_string())
            .join(MANIFEST_FILE);
        let read = tokio::task::spawn_blocking(move || {
            manifest
                .try_exists()?
                .then(|| read_manifest_file(&manifest))
                .transpose()
        })
        .await??;
        if read.is_some_and(|manifest| manifest.agent_id != agent_id) {
            return Err(LocalHostError::InvalidRequest(
                "session belongs to a different agent".to_owned(),
            ));
        }
        if self.config.claims.open_elsewhere(session_uuid) {
            return Err(LocalHostError::InvalidRequest(
                "session is open in this process; close it first".to_owned(),
            ));
        }
        Ok(())
    }
}
