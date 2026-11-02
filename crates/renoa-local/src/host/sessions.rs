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
    agent_session::{AgentSessionStorage, claim::Claim},
    host::{catalog::HostCatalogError, models::validate_selection},
    host_storage::{
        OpenedSessionStorage, SessionPublication, create_session_storage, delete_session_storage,
        open_session_storage,
    },
    plugins::host::{memory::note_pass, session_context},
    selection::{RuntimeSelection, SELECTION_FILE, read_selection},
    speaker::ToolAccess,
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
        let opened = self
            .config
            .claims
            .open(session_uuid, async {
                if self
                    .config
                    .sessions
                    .join(session_id.to_string())
                    .try_exists()?
                {
                    return self.load_unshared(agent_id, session_uuid, cwd).await;
                }
                self.create_session_for_definition(&definition, cwd, session_uuid)
                    .await
            })
            .await?;
        owned(opened, agent_id, cwd)
    }

    /// Opens a stored conversation of `agent_id` for a note pass that
    /// `cancellation` stops, claimed until the returned handle drops; `None`
    /// while a surface holds it or passes are stopped. A surface that claims
    /// it meanwhile cancels the pass and waits for it (see
    /// [`claim`](crate::agent_session::claim)).
    ///
    /// # Errors
    /// Returns a foreign-agent rejection, or identity, workspace, provider,
    /// runtime, or storage incompatibility.
    pub(crate) async fn open_for_note_pass(
        &self,
        agent_id: AgentId,
        session_uuid: Uuid,
        cwd: &Path,
        cancellation: &tokio_util::sync::CancellationToken,
    ) -> Result<Option<ClaimedSession>, LocalHostError> {
        let Some(claim) = self.config.claims.note_pass(session_uuid, cancellation) else {
            return Ok(None);
        };
        let session = self
            .load_session_for_agent(agent_id, session_uuid, cwd)
            .await?;
        Ok(Some(ClaimedSession {
            session,
            _claim: claim,
        }))
    }

    /// Stops `renoa.memory`'s runs in this process, for a node that is
    /// draining before it stops: every running note pass is cancelled and
    /// settles as not run, its notes kept, no new pass starts, and a nightly
    /// run stops before its next job, so no pass's command is left unfinished
    /// under the runtime a deploy replaces. A run stopped part way is not to
    /// be recorded: the next scheduler hands it on and it resumes.
    pub fn stop_memory_runs(&self) {
        self.config.claims.stop_passes();
    }

    /// Whether [`Self::stop_memory_runs`] was called.
    #[must_use]
    pub fn memory_runs_stopped(&self) -> bool {
        self.config.claims.passes_stopped()
    }

    /// Whether a session with this id is stored.
    pub(crate) fn has_session(&self, session_uuid: Uuid) -> std::io::Result<bool> {
        self.config
            .sessions
            .join(SessionId::from_uuid(session_uuid).to_string())
            .try_exists()
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
        let preference = definition.model();
        let initial_provider = preference
            .map(|model| model.provider)
            .or(definition.provider_restriction())
            .unwrap_or(self.config.initial_provider);
        let model_id = preference.map_or(self.config.initial_model.as_str(), |model| {
            model.model.as_str()
        });
        let model = require_model(&models, initial_provider, model_id, "agent")?;
        let reasoning = initial_reasoning(
            model,
            preference.map_or(self.config.initial_reasoning, |model| model.reasoning),
        )?;
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
                tools: ToolAccess::Granted,
            },
        )
        .await?;
        let selection = RuntimeSelection {
            provider: initial_provider,
            model: model.id().to_owned(),
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
                session_context_pin: directory.join(session_context::PIN_FILE),
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
        let opened = self
            .config
            .claims
            .open(
                session_uuid,
                self.load_unshared(agent_id, session_uuid, cwd),
            )
            .await?;
        owned(opened, agent_id, cwd)
    }

    /// Opens a new instance of a stored session; only through
    /// [`Claims::open`](crate::agent_session::claim::Claims::open), which
    /// keeps one per session.
    async fn load_unshared(
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
                session_context_pin: directory.join(session_context::PIN_FILE),
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
        self.refuse_foreign(agent_id, session_uuid).await?;
        let _claim = self.config.claims.surface(session_uuid).await?;
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
    /// deleted; a note pass reviewing it is cancelled and waited for first,
    /// as for a turn, and the session goes with what the pass kept of it in
    /// one catalog transaction, so a refusal deletes neither.
    pub async fn delete_session(
        &self,
        agent_id: AgentId,
        session_uuid: Uuid,
    ) -> Result<(), LocalHostError> {
        self.refuse_foreign(agent_id, session_uuid).await?;
        let _claim = self.config.claims.surface(session_uuid).await?;
        let sessions = self.config.sessions.clone();
        let database = self.config.database.clone();
        let session_id = SessionId::from_uuid(session_uuid);
        tokio::task::spawn_blocking(move || {
            let mut catalog = crate::host::catalog::open_verified(&database)?;
            let transaction = catalog.transaction().map_err(HostCatalogError::from)?;
            note_pass::store::forget(&transaction, session_uuid)?;
            delete_session_storage(&sessions, agent_id, session_id)?;
            transaction.commit().map_err(HostCatalogError::from)?;
            Ok::<_, LocalHostError>(())
        })
        .await??;
        self.config.claims.forget(session_uuid);
        let skills = self.config.skill_store.clone();
        tokio::task::spawn_blocking(move || skills.remove_session(session_id)).await??;
        Ok(())
    }
}

impl LocalHost {
    /// Refuses a stored session of another agent before a claim on it cancels
    /// that agent's note pass; a missing session is left to the caller. The
    /// opening that follows checks its manifest again.
    ///
    /// # Errors
    ///
    /// Returns a foreign-agent rejection or a metadata failure.
    pub(super) async fn refuse_foreign(
        &self,
        agent_id: AgentId,
        session_uuid: Uuid,
    ) -> Result<(), LocalHostError> {
        let manifest = self
            .config
            .sessions
            .join(SessionId::from_uuid(session_uuid).to_string())
            .join(crate::host_storage::MANIFEST_FILE);
        let read = tokio::task::spawn_blocking(move || {
            manifest
                .try_exists()?
                .then(|| crate::host_storage::read_manifest_file(&manifest))
                .transpose()
        })
        .await??;
        match read {
            Some(manifest) if manifest.agent_id != agent_id => Err(LocalHostError::InvalidRequest(
                "session belongs to a different agent".to_owned(),
            )),
            _ => Ok(()),
        }
    }
}

/// A session opened for a note pass, claimed for it until dropped; the
/// session goes before the claim.
pub(crate) struct ClaimedSession {
    session: Arc<AgentSession>,
    _claim: Claim,
}

impl std::ops::Deref for ClaimedSession {
    type Target = AgentSession;

    fn deref(&self) -> &AgentSession {
        &self.session
    }
}

/// `session` when `agent_id` owns it and it is bound to `cwd`: the checks
/// opening its storage makes, again for an instance already open.
fn owned(
    session: Arc<AgentSession>,
    agent_id: AgentId,
    cwd: &Path,
) -> Result<Arc<AgentSession>, LocalHostError> {
    if session.agent_id() != agent_id {
        return Err(LocalHostError::InvalidRequest(
            "session belongs to a different agent".to_owned(),
        ));
    }
    if session.workspace() != std::fs::canonicalize(cwd)? {
        return Err(LocalHostError::InvalidRequest(
            "session workspace differs from its durable binding".to_owned(),
        ));
    }
    Ok(session)
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
