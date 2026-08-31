use std::{path::Path, sync::Arc};

use renoa_kernel::{AgentId, SessionId};
use uuid::Uuid;

use super::{
    LocalHost, LocalHostError, RuntimeRequest, initial_reasoning, require_model, resolve_runtime,
};
use crate::{
    AgentProfileId, AgentSession, LocalSession, LocalWorkspace,
    agent_session::AgentSessionStorage,
    host::models::validate_selection,
    host_storage::{
        KERNEL_DATABASE, MANIFEST_FILE, SessionPublication, create_session_storage,
        delete_session_storage, load_session_after_handoff, read_manifest,
    },
    selection::{RuntimeSelection, SELECTION_FILE, read_selection},
    trace::{TRACE_DATABASE, TraceStore},
};

impl LocalHost {
    /// Resolves and atomically publishes one new session for an exact profile.
    ///
    /// # Errors
    ///
    /// Returns provider, workspace, runtime, or durable storage failures.
    pub async fn create_session(
        &self,
        profile_id: &AgentProfileId,
        cwd: &Path,
    ) -> Result<Arc<AgentSession>, LocalHostError> {
        self.create_session_with_id(profile_id, cwd, Uuid::new_v4(), false)
            .await
    }

    /// Creates or reloads one caller-identified session for an exact profile and workspace.
    ///
    /// This is the durable surface-admission path: retrying after process loss
    /// resolves the same session instead of creating an orphan replacement.
    ///
    /// # Errors
    ///
    /// Returns provider, workspace, identity, runtime, or durable storage failures.
    pub async fn ensure_session(
        &self,
        profile_id: &AgentProfileId,
        cwd: &Path,
        session_uuid: Uuid,
    ) -> Result<Arc<AgentSession>, LocalHostError> {
        self.create_session_with_id(profile_id, cwd, session_uuid, true)
            .await
    }

    async fn create_session_with_id(
        &self,
        profile_id: &AgentProfileId,
        cwd: &Path,
        session_uuid: Uuid,
        load_existing: bool,
    ) -> Result<Arc<AgentSession>, LocalHostError> {
        require_absolute(cwd)?;
        let session_id = SessionId::from_uuid(session_uuid);
        if load_existing
            && self
                .config
                .sessions
                .join(session_id.to_string())
                .try_exists()?
        {
            return self
                .load_session_for_profile(profile_id, session_uuid, cwd)
                .await;
        }
        let profile = self.profile(profile_id)?.clone();
        let workspace = LocalWorkspace::open(cwd)?;
        let workspace_path = std::fs::canonicalize(cwd)?;
        let models = self.models().await?;
        let reasoning = initial_reasoning(
            &models,
            self.config.initial_provider,
            &self.config.initial_model,
        )?;
        let model = require_model(
            &models,
            self.config.initial_provider,
            &self.config.initial_model,
            "configured",
        )?;
        let agent_id = AgentId::new();
        resolve_runtime(
            &self.config,
            RuntimeRequest {
                profile: &profile,
                session_id,
                command_id: None,
                model,
                reasoning,
                workspace: &workspace,
                events: None,
            },
        )
        .await?;
        let selection = RuntimeSelection {
            provider: self.config.initial_provider,
            model: self.config.initial_model.clone(),
            reasoning,
        };
        let sessions = self.config.sessions.clone();
        let stored_selection = selection.clone();
        let stored_workspace = workspace_path.clone();
        let stored_profile = profile.id().clone();
        let publication = tokio::task::spawn_blocking(move || {
            create_session_storage(
                &sessions,
                stored_profile,
                agent_id,
                session_id,
                stored_workspace,
                &stored_selection,
            )
        })
        .await??;
        let directory = match publication {
            SessionPublication::Created(directory) => directory,
            SessionPublication::Existing if load_existing => {
                return self
                    .load_session_for_profile(profile_id, session_uuid, cwd)
                    .await;
            }
            SessionPublication::Existing => {
                return Err(LocalHostError::InvalidRequest(
                    "generated Agent session identity already exists".to_owned(),
                ));
            }
        };
        let kernel = load_session_after_handoff(&directory.join(KERNEL_DATABASE), session_id)?;
        let trace = TraceStore::open(
            directory.join(TRACE_DATABASE),
            session_id,
            agent_id,
            profile.id(),
        )?;
        Ok(Arc::new(AgentSession::new(
            session_uuid,
            profile.id().clone(),
            Arc::clone(&self.config),
            AgentSessionStorage {
                kernel,
                workspace: workspace_path,
                selection_path: directory.join(SELECTION_FILE),
                trace,
            },
            models,
            selection,
        )))
    }

    async fn load_session_for_profile(
        &self,
        profile_id: &AgentProfileId,
        session_uuid: Uuid,
        cwd: &Path,
    ) -> Result<Arc<AgentSession>, LocalHostError> {
        let session = self.load_session(session_uuid, cwd).await?;
        if session.profile_id() != profile_id {
            return Err(LocalHostError::InvalidRequest(format!(
                "session {session_uuid} belongs to profile `{}`, not requested profile `{profile_id}`",
                session.profile_id()
            )));
        }
        Ok(session)
    }

    /// Reloads one exact Agent session and its durable profile/workspace binding.
    ///
    /// # Errors
    ///
    /// Returns identity, workspace, provider, runtime, or storage incompatibility.
    pub async fn load_session(
        &self,
        session_uuid: Uuid,
        cwd: &Path,
    ) -> Result<Arc<AgentSession>, LocalHostError> {
        require_absolute(cwd)?;
        let session_id = SessionId::from_uuid(session_uuid);
        let directory = self.config.sessions.join(session_id.to_string());
        let manifest = read_manifest(directory.join(MANIFEST_FILE)).await?;
        if manifest.session_id != session_id {
            return Err(LocalHostError::InvalidRequest(
                "session metadata does not match the requested Agent session".to_owned(),
            ));
        }
        self.profile(&manifest.profile)?;
        let requested_workspace = std::fs::canonicalize(cwd)?;
        if manifest.workspace != requested_workspace {
            return Err(LocalHostError::InvalidRequest(
                "session workspace differs from its durable binding".to_owned(),
            ));
        }
        let kernel = LocalSession::load(directory.join(KERNEL_DATABASE), session_id)?;
        if kernel.agent_id() != manifest.agent_id {
            return Err(LocalHostError::InvalidRequest(
                "session metadata differs from its kernel agent binding".to_owned(),
            ));
        }
        let selection_path = directory.join(SELECTION_FILE);
        let trace = TraceStore::open(
            directory.join(TRACE_DATABASE),
            session_id,
            manifest.agent_id,
            &manifest.profile,
        )?;
        let selection = read_selection(selection_path.clone()).await?;
        if !self.config.providers.contains(&selection.provider) {
            return Err(LocalHostError::Configuration(format!(
                "session requires the {} provider, but it is not enabled",
                selection.provider
            )));
        }
        let models = self.models().await?;
        validate_selection(&models, &selection)?;
        LocalWorkspace::open(&requested_workspace)?;
        Ok(Arc::new(AgentSession::new(
            session_uuid,
            manifest.profile,
            Arc::clone(&self.config),
            AgentSessionStorage {
                kernel,
                workspace: requested_workspace,
                selection_path,
                trace,
            },
            models,
            selection,
        )))
    }

    /// Permanently removes one closed Agent session from durable Host storage.
    ///
    /// Deleting a missing session succeeds so a retried ACP request is safe.
    ///
    /// # Errors
    ///
    /// Returns an ownership, identity, metadata, or storage failure. A session
    /// still owned by any process cannot be deleted.
    pub async fn delete_session(&self, session_uuid: Uuid) -> Result<(), LocalHostError> {
        let sessions = self.config.sessions.clone();
        let session_id = SessionId::from_uuid(session_uuid);
        tokio::task::spawn_blocking(move || delete_session_storage(&sessions, session_id))
            .await??;
        let skills = self.config.skill_store.clone();
        tokio::task::spawn_blocking(move || skills.remove_session(session_id)).await??;
        Ok(())
    }
}

fn require_absolute(cwd: &Path) -> Result<(), LocalHostError> {
    if cwd.is_absolute() {
        Ok(())
    } else {
        Err(LocalHostError::InvalidRequest(
            "session cwd must be an absolute path".to_owned(),
        ))
    }
}
