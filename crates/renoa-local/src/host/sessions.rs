use std::{path::Path, sync::Arc};

use renoa_kernel::{AgentId, SessionId};
use uuid::Uuid;

use super::{
    LocalHost, LocalHostError, RuntimeRequest,
    definition::{ResolvedAgentDefinition, resolve_definition},
    discover_models_for, initial_reasoning, require_model, resolve_runtime,
};
use crate::{
    AgentSession, LocalWorkspace,
    agent_session::AgentSessionStorage,
    host::models::validate_selection,
    host_storage::{
        OpenedSessionStorage, SessionPublication, create_session_storage, delete_session_storage,
        open_session_storage,
    },
    selection::{RuntimeSelection, SELECTION_FILE, read_selection},
    trace::{TRACE_DATABASE, TraceStore},
};

impl LocalHost {
    /// Creates or reloads a caller-identified conversation for an existing agent.
    ///
    /// Several sessions can share an agent identity while retaining separate kernel
    /// history, workspaces, selections, and execution ownership. Retrying a session
    /// identity cannot move it to a different agent or workspace.
    ///
    /// # Errors
    /// Returns missing agent, identity/binding, runtime, or storage errors.
    pub async fn ensure_agent_session(
        &self,
        agent_id: AgentId,
        cwd: &Path,
        session_uuid: Uuid,
    ) -> Result<Arc<AgentSession>, LocalHostError> {
        require_absolute(cwd)?;
        let definition = resolve_definition(&self.config, agent_id).await?;
        let session_id = SessionId::from_uuid(session_uuid);
        if self
            .config
            .sessions
            .join(session_id.to_string())
            .try_exists()?
        {
            return self
                .load_session_for_agent(agent_id, session_uuid, cwd)
                .await;
        }
        self.create_session_for_definition(&definition, cwd, session_uuid)
            .await
    }

    async fn create_session_for_definition(
        &self,
        definition: &ResolvedAgentDefinition,
        cwd: &Path,
        session_uuid: Uuid,
    ) -> Result<Arc<AgentSession>, LocalHostError> {
        let agent_id = definition.agent_id();
        let session_id = SessionId::from_uuid(session_uuid);
        let workspace = LocalWorkspace::open(cwd)?;
        let workspace_path = std::fs::canonicalize(cwd)?;
        let models = discover_models_for(&self.config, definition.provider_restriction()).await?;
        let initial_provider = definition
            .provider_restriction()
            .unwrap_or(self.config.initial_provider);
        let model = require_model(
            &models,
            initial_provider,
            &self.config.initial_model,
            "configured",
        )?;
        let reasoning = initial_reasoning(model, self.config.initial_reasoning)?;
        resolve_runtime(
            &self.config,
            RuntimeRequest {
                definition,
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
            provider: initial_provider,
            model: self.config.initial_model.clone(),
            reasoning,
        };
        let sessions = self.config.sessions.clone();
        let stored_selection = selection.clone();
        let stored_workspace = workspace_path.clone();
        let publication = tokio::task::spawn_blocking(move || {
            create_session_storage(
                &sessions,
                agent_id,
                session_id,
                stored_workspace,
                &stored_selection,
            )
        })
        .await??;
        let stored = match publication {
            SessionPublication::Created(stored) => stored,
            SessionPublication::Existing(stored) => {
                return self.assemble_session(session_uuid, stored).await;
            }
        };
        let OpenedSessionStorage {
            directory, kernel, ..
        } = stored;
        let trace = TraceStore::open(directory.join(TRACE_DATABASE), session_id, agent_id)?;
        Ok(Arc::new(AgentSession::new(
            session_uuid,
            agent_id,
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

    /// Reloads one exact Agent session only when the requesting agent owns it.
    ///
    /// A session bound to a different agent is refused from its manifest before
    /// its kernel is opened, its definition resolved, its trace recovered, its
    /// models discovered, or its workspace opened, so every by-id read path can
    /// inherit the ownership check.
    ///
    /// # Errors
    ///
    /// Returns a foreign-agent rejection, or identity, workspace, provider,
    /// runtime, or storage incompatibility.
    pub async fn load_session_for_agent(
        &self,
        agent_id: AgentId,
        session_uuid: Uuid,
        cwd: &Path,
    ) -> Result<Arc<AgentSession>, LocalHostError> {
        let stored = self
            .load_session_storage(agent_id, session_uuid, cwd)
            .await?;
        self.assemble_session(session_uuid, stored).await
    }

    async fn assemble_session(
        &self,
        session_uuid: Uuid,
        stored: OpenedSessionStorage,
    ) -> Result<Arc<AgentSession>, LocalHostError> {
        let OpenedSessionStorage {
            directory,
            manifest,
            kernel,
        } = stored;
        let session_id = manifest.session_id;
        let agent_id = manifest.agent_id;
        let definition = resolve_definition(&self.config, agent_id).await?;
        let requested_workspace = manifest.workspace.clone();
        let selection_path = directory.join(SELECTION_FILE);
        let trace = TraceStore::open(directory.join(TRACE_DATABASE), session_id, agent_id)?;
        let selection = read_selection(selection_path.clone()).await?;
        if !self.config.providers.contains(&selection.provider) {
            return Err(LocalHostError::Configuration(format!(
                "session requires the {} provider, but it is not enabled",
                selection.provider
            )));
        }
        if let Some(required) = definition.provider_restriction()
            && selection.provider != required
        {
            return Err(LocalHostError::Configuration(format!(
                "session agent {agent_id} permits only the {} provider, but its saved model uses {}",
                required.name(),
                selection.provider.name()
            )));
        }
        let models = discover_models_for(&self.config, definition.provider_restriction()).await?;
        validate_selection(&models, &selection)?;
        LocalWorkspace::open(&requested_workspace)?;
        Ok(Arc::new(AgentSession::new(
            session_uuid,
            agent_id,
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

    /// Resolves cancellation without assembling an executable session.
    ///
    /// The caller must durably retain cancellation for this exact request. An
    /// absent session/request returns `Cancelled` without creating kernel data;
    /// settled outcomes replay and unfinished requests retain durable cancellation
    /// for their bound runtime, returning `None`. `content: None` means compaction.
    ///
    /// # Errors
    ///
    /// Returns agent/workspace binding, ownership, request identity, or storage failures.
    pub async fn cancel_before_execution(
        &self,
        agent_id: AgentId,
        cwd: &Path,
        session_uuid: Uuid,
        request_id: Uuid,
        content: Option<&[renoa_agent::ContentBlock]>,
    ) -> Result<Option<crate::LocalTurnOutcome>, LocalHostError> {
        require_absolute(cwd)?;
        if self.agent_definition(agent_id).await?.is_none() {
            return Err(LocalHostError::AgentNotFound(agent_id));
        }
        if !self
            .config
            .sessions
            .join(session_uuid.to_string())
            .try_exists()?
        {
            return Ok(Some(crate::LocalTurnOutcome::Cancelled));
        }
        let stored = self
            .load_session_storage(agent_id, session_uuid, cwd)
            .await?;
        Ok(stored.kernel.cancel_before_execution(
            renoa_kernel::CommandId::from_uuid(request_id),
            content,
            renoa_kernel::CancellationId::from_uuid(request_id),
        )?)
    }

    /// Loads one stored session, asserting its owning agent.
    ///
    /// `expected_agent` is compared with the manifest immediately after it is
    /// read, so a refused foreign session never reaches its kernel, definition,
    /// trace, models, or workspace.
    ///
    /// # Errors
    ///
    /// Returns a foreign-agent rejection, or identity, workspace binding,
    /// ownership, or storage failures.
    pub(super) async fn load_session_storage(
        &self,
        expected_agent: AgentId,
        session_uuid: Uuid,
        cwd: &Path,
    ) -> Result<OpenedSessionStorage, LocalHostError> {
        require_absolute(cwd)?;
        let session_id = SessionId::from_uuid(session_uuid);
        let sessions = self.config.sessions.clone();
        let workspace = cwd.to_owned();
        tokio::task::spawn_blocking(move || {
            open_session_storage(&sessions, expected_agent, session_id, &workspace)
        })
        .await?
    }

    /// Permanently removes one closed Agent session owned by `agent_id`.
    ///
    /// A session bound to a different agent is refused from the manifest its
    /// deletion already reads, before any storage is removed. Deleting a
    /// missing session succeeds so a retried ACP request is safe.
    ///
    /// # Errors
    ///
    /// Returns a foreign-agent rejection, or an ownership, identity, metadata,
    /// or storage failure. A session still owned by any process cannot be
    /// deleted.
    pub async fn delete_session(
        &self,
        agent_id: AgentId,
        session_uuid: Uuid,
    ) -> Result<(), LocalHostError> {
        let sessions = self.config.sessions.clone();
        let session_id = SessionId::from_uuid(session_uuid);
        tokio::task::spawn_blocking(move || {
            delete_session_storage(&sessions, agent_id, session_id)
        })
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
