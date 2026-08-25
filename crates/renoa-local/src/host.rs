use std::{
    collections::HashSet,
    path::{Path, PathBuf},
    sync::Arc,
};

use renoa_agent::AgentEventSink;
use renoa_kernel::{AgentId, SessionId};
use thiserror::Error;
use uuid::Uuid;

use crate::alpha_session::AlphaSessionStorage;
use crate::{
    AlphaError, AlphaSession, LocalRuntimeConfig, LocalRuntimeError, LocalSession,
    LocalSessionError, LocalWorkspace, LocalWorkspaceError, ModelBridgeError, ModelChoice,
    ModelProvider, ReasoningLevel, build_local_runtime, build_local_runtime_with_events,
    discover_models,
    host_storage::{
        KERNEL_DATABASE, MANIFEST_FILE, create_session_storage, delete_session_storage,
        read_manifest,
    },
    selection::{RuntimeSelection, SELECTION_FILE, read_selection},
    trace::{TRACE_DATABASE, TraceError, TraceStore},
};

/// Process-local configuration used to compose Renoa Alpha sessions.
pub struct LocalHost {
    config: Arc<HostConfig>,
}

pub(crate) struct HostConfig {
    pub(crate) sessions: PathBuf,
    pub(crate) bridge: PathBuf,
    pub(crate) providers: Vec<ModelProvider>,
    pub(crate) initial_provider: ModelProvider,
    pub(crate) initial_model: String,
    pub(crate) credential_store: PathBuf,
}

/// Failure while composing, storing, or running a local Agent instance.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum LocalHostError {
    #[error("invalid local Host request: {0}")]
    InvalidRequest(String),
    #[error("invalid local Host configuration: {0}")]
    Configuration(String),
    #[error("local Host session storage failed: {0}")]
    Io(#[from] std::io::Error),
    #[error("local Host session metadata is invalid: {0}")]
    Metadata(#[from] serde_json::Error),
    #[error(transparent)]
    Workspace(#[from] LocalWorkspaceError),
    #[error(transparent)]
    Runtime(#[from] LocalRuntimeError),
    #[error(transparent)]
    Model(#[from] ModelBridgeError),
    #[error(transparent)]
    Alpha(#[from] AlphaError),
    #[error(transparent)]
    Session(#[from] LocalSessionError),
    #[error("local Host background storage task failed: {0}")]
    Background(#[from] tokio::task::JoinError),
    #[error("local Host session state lock was poisoned")]
    StatePoisoned,
    #[error("local Host trace failed: {0}")]
    Trace(String),
    #[error("session creation failed: {source}; staging cleanup also failed: {cleanup}")]
    SessionCreationCleanup {
        #[source]
        source: Box<LocalHostError>,
        cleanup: std::io::Error,
    },
}

impl From<TraceError> for LocalHostError {
    fn from(error: TraceError) -> Self {
        Self::Trace(error.to_string())
    }
}

impl LocalHost {
    /// Creates the local Host around its enabled providers and durable session root.
    ///
    /// # Errors
    ///
    /// Returns a storage error when the session root cannot be created or resolved.
    pub fn new(
        sessions: impl Into<PathBuf>,
        bridge: impl Into<PathBuf>,
        providers: Vec<ModelProvider>,
        initial_provider: ModelProvider,
        initial_model: impl Into<String>,
        credential_store: impl Into<PathBuf>,
    ) -> Result<Self, LocalHostError> {
        if providers.is_empty() {
            return Err(LocalHostError::Configuration(
                "at least one model provider must be enabled".to_owned(),
            ));
        }
        if providers.iter().copied().collect::<HashSet<_>>().len() != providers.len() {
            return Err(LocalHostError::Configuration(
                "enabled model providers must be unique".to_owned(),
            ));
        }
        if !providers.contains(&initial_provider) {
            return Err(LocalHostError::Configuration(format!(
                "default {initial_provider} provider is not enabled"
            )));
        }
        let sessions = sessions.into();
        std::fs::create_dir_all(&sessions)?;
        let sessions = std::fs::canonicalize(sessions)?;
        Ok(Self {
            config: Arc::new(HostConfig {
                sessions,
                bridge: bridge.into(),
                providers,
                initial_provider,
                initial_model: initial_model.into(),
                credential_store: credential_store.into(),
            }),
        })
    }

    /// Resolves and atomically publishes one new Alpha session.
    ///
    /// # Errors
    ///
    /// Returns provider, workspace, runtime, or durable storage failures.
    pub async fn create_alpha_session(
        &self,
        cwd: &Path,
    ) -> Result<Arc<AlphaSession>, LocalHostError> {
        require_absolute(cwd)?;
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
        self.resolve_runtime(model, reasoning, &workspace, None)
            .await?;
        let agent_id = AgentId::new();
        let session_uuid = Uuid::new_v4();
        let session_id = SessionId::from_uuid(session_uuid);
        let selection = RuntimeSelection {
            provider: self.config.initial_provider,
            model: self.config.initial_model.clone(),
            reasoning,
        };
        let sessions = self.config.sessions.clone();
        let stored_selection = selection.clone();
        let stored_workspace = workspace_path.clone();
        let directory = tokio::task::spawn_blocking(move || {
            create_session_storage(
                &sessions,
                agent_id,
                session_id,
                stored_workspace,
                &stored_selection,
            )
        })
        .await??;
        let kernel = LocalSession::load(directory.join(KERNEL_DATABASE), session_id)?;
        let trace = TraceStore::open(directory.join(TRACE_DATABASE), session_id)?;
        Ok(Arc::new(AlphaSession::new(
            session_uuid,
            Arc::clone(&self.config),
            AlphaSessionStorage {
                kernel,
                workspace: workspace_path,
                selection_path: directory.join(SELECTION_FILE),
                trace,
            },
            models,
            selection,
        )))
    }

    /// Reloads one exact Alpha session and its durable workspace/runtime binding.
    ///
    /// # Errors
    ///
    /// Returns identity, workspace, provider, runtime, or storage incompatibility.
    pub async fn load_alpha_session(
        &self,
        session_uuid: Uuid,
        cwd: &Path,
    ) -> Result<Arc<AlphaSession>, LocalHostError> {
        require_absolute(cwd)?;
        let session_id = SessionId::from_uuid(session_uuid);
        let directory = self.config.sessions.join(session_id.to_string());
        let manifest = read_manifest(directory.join(MANIFEST_FILE)).await?;
        if manifest.session_id != session_id {
            return Err(LocalHostError::InvalidRequest(
                "session metadata does not match the requested Alpha session".to_owned(),
            ));
        }
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
        let trace = TraceStore::open(directory.join(TRACE_DATABASE), session_id)?;
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
        Ok(Arc::new(AlphaSession::new(
            session_uuid,
            Arc::clone(&self.config),
            AlphaSessionStorage {
                kernel,
                workspace: requested_workspace,
                selection_path,
                trace,
            },
            models,
            selection,
        )))
    }

    /// Permanently removes one closed Alpha session from durable Host storage.
    ///
    /// Deleting a missing session succeeds so a retried ACP request is safe.
    ///
    /// # Errors
    ///
    /// Returns an ownership, identity, metadata, or storage failure. A session
    /// still owned by any process cannot be deleted.
    pub async fn delete_alpha_session(&self, session_uuid: Uuid) -> Result<(), LocalHostError> {
        let sessions = self.config.sessions.clone();
        let session_id = SessionId::from_uuid(session_uuid);
        tokio::task::spawn_blocking(move || delete_session_storage(&sessions, session_id)).await?
    }

    async fn models(&self) -> Result<Vec<ModelChoice>, LocalHostError> {
        let mut models = Vec::new();
        for provider in &self.config.providers {
            models.extend(
                discover_models(
                    self.config.bridge.clone(),
                    *provider,
                    self.config.credential_store.clone(),
                )
                .await?,
            );
        }
        Ok(models)
    }

    async fn resolve_runtime(
        &self,
        model: &ModelChoice,
        reasoning: ReasoningLevel,
        workspace: &LocalWorkspace,
        events: Option<Arc<dyn AgentEventSink>>,
    ) -> Result<renoa_kernel::Runtime, LocalHostError> {
        resolve_runtime(&self.config, model, reasoning, workspace, events).await
    }
}

pub(crate) async fn resolve_runtime(
    host: &HostConfig,
    model: &ModelChoice,
    reasoning: ReasoningLevel,
    workspace: &LocalWorkspace,
    events: Option<Arc<dyn AgentEventSink>>,
) -> Result<renoa_kernel::Runtime, LocalHostError> {
    let config = LocalRuntimeConfig::for_alpha(
        host.bridge.clone(),
        model.provider().as_str(),
        model.id(),
        host.credential_store.clone(),
        workspace,
    )?
    .with_discovered_model(model)
    .with_reasoning(reasoning);
    match events {
        Some(events) => Ok(build_local_runtime_with_events(config, workspace, events).await?),
        None => Ok(build_local_runtime(config, workspace).await?),
    }
}

pub(crate) fn initial_reasoning(
    models: &[ModelChoice],
    provider: ModelProvider,
    configured_model: &str,
) -> Result<ReasoningLevel, LocalHostError> {
    let model = require_model(models, provider, configured_model, "configured")?;
    model.default_reasoning().ok_or_else(|| {
        LocalHostError::Configuration(format!(
            "configured {configured_model} model has no supported reasoning level"
        ))
    })
}

pub(crate) fn selected_model<'a>(
    models: &'a [ModelChoice],
    provider: ModelProvider,
    id: &str,
) -> Option<&'a ModelChoice> {
    models
        .iter()
        .find(|model| model.provider() == provider && model.id() == id)
}

pub(crate) fn selected_model_by_selection_id<'a>(
    models: &'a [ModelChoice],
    selection_id: &str,
) -> Option<&'a ModelChoice> {
    models
        .iter()
        .find(|model| model.selection_id() == selection_id)
}

pub(crate) fn require_model<'a>(
    models: &'a [ModelChoice],
    provider: ModelProvider,
    id: &str,
    source: &str,
) -> Result<&'a ModelChoice, LocalHostError> {
    selected_model(models, provider, id).ok_or_else(|| {
        LocalHostError::Configuration(format!(
            "{source} {provider}/{id} model is not available from the authenticated provider"
        ))
    })
}

fn validate_selection<'a>(
    models: &'a [ModelChoice],
    selection: &RuntimeSelection,
) -> Result<&'a ModelChoice, LocalHostError> {
    let model = require_model(models, selection.provider, &selection.model, "saved")?;
    if !model.reasoning_levels().contains(&selection.reasoning) {
        return Err(LocalHostError::Configuration(format!(
            "saved {} model no longer supports {} reasoning",
            selection.model,
            selection.reasoning.as_str()
        )));
    }
    Ok(model)
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
