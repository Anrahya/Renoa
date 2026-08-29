use std::{
    collections::HashSet,
    path::{Path, PathBuf},
    sync::Arc,
};

use renoa_agent::AgentEventSink;
use renoa_kernel::{AgentId, CommandId, SessionId};
use thiserror::Error;
use uuid::Uuid;

pub(crate) mod catalog;
mod extensions;
mod mcp;
mod models;
#[cfg(test)]
mod skill_tests;

use crate::alpha_session::AlphaSessionStorage;
use crate::{
    AlphaError, AlphaSession, LocalRuntimeConfig, LocalRuntimeError, LocalSession,
    LocalSessionError, LocalWorkspace, LocalWorkspaceError, ModelBridgeError, ModelChoice,
    ModelProvider, ReasoningLevel,
    host_storage::{
        KERNEL_DATABASE, MANIFEST_FILE, create_session_storage, delete_session_storage,
        read_manifest,
    },
    mcp::{
        McpCatalogStore, McpCredentialResolver, McpHostError, alpha_registry_bindings,
        resolve_adapter,
    },
    plugins::{
        OfficialRegistry, PLUGIN_STORE_DIRECTORY, PluginError, PluginManager, alpha_plugin_binding,
    },
    runtime::build_composed_local_runtime,
    selection::{RuntimeSelection, SELECTION_FILE, read_selection},
    skills::{
        SkillError, SkillStore, alpha_skill_bindings, default_global_source, runtime_context,
        store_path,
    },
    trace::{TRACE_DATABASE, TraceError, TraceStore},
};

use models::validate_selection;
pub(crate) use models::{
    discover_enabled_models, initial_reasoning, require_model, selected_model_by_selection_id,
};

/// Process-local configuration used to compose Renoa Alpha sessions.
pub struct LocalHost {
    config: Arc<HostConfig>,
}

/// Optional replaceable process adapters used by the local Host.
#[derive(Clone, Copy, Default)]
pub struct LocalHostAdapters<'a> {
    mcp: Option<&'a Path>,
    mcp_registry: Option<&'a Path>,
}

impl<'a> LocalHostAdapters<'a> {
    /// Selects the MCP runtime adapter.
    #[must_use]
    pub const fn new(mcp: Option<&'a Path>) -> Self {
        Self {
            mcp,
            mcp_registry: None,
        }
    }

    /// Selects the official MCP Registry discovery adapter.
    #[must_use]
    pub const fn with_mcp_registry(mut self, registry: Option<&'a Path>) -> Self {
        self.mcp_registry = registry;
        self
    }
}

pub(crate) struct HostConfig {
    pub(crate) sessions: PathBuf,
    pub(crate) bridge: PathBuf,
    pub(crate) providers: Vec<ModelProvider>,
    pub(crate) initial_provider: ModelProvider,
    pub(crate) initial_model: String,
    pub(crate) credential_store: PathBuf,
    pub(crate) mcp_catalog: McpCatalogStore,
    pub(crate) mcp_adapter: Option<PathBuf>,
    pub(crate) mcp_credentials: McpCredentialResolver,
    pub(crate) skill_store: SkillStore,
    pub(crate) plugins: PluginManager,
}

struct HostInitialization {
    data_directory: PathBuf,
    bridge: PathBuf,
    providers: Vec<ModelProvider>,
    initial_provider: ModelProvider,
    initial_model: String,
    credential_store: PathBuf,
    mcp_adapter: Option<PathBuf>,
    mcp_registry_adapter: Option<PathBuf>,
    global_skill_source: Option<PathBuf>,
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
    #[error(transparent)]
    Mcp(#[from] McpHostError),
    #[error(transparent)]
    Skill(#[from] SkillError),
    #[error(transparent)]
    Plugin(#[from] PluginError),
    #[error(transparent)]
    HostCatalog(#[from] catalog::HostCatalogError),
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
    /// Creates the local Host around its durable data root and enabled providers.
    ///
    /// # Errors
    ///
    /// Returns when the data root, session root, MCP adapter, or Host catalog
    /// cannot be initialized.
    pub fn new(
        data_directory: impl Into<PathBuf>,
        bridge: impl Into<PathBuf>,
        providers: Vec<ModelProvider>,
        initial_provider: ModelProvider,
        initial_model: impl Into<String>,
        credential_store: impl Into<PathBuf>,
        adapters: LocalHostAdapters<'_>,
    ) -> Result<Self, LocalHostError> {
        let mcp_adapter = adapters
            .mcp
            .map(resolve_adapter)
            .transpose()
            .map_err(McpHostError::from)?;
        let mcp_registry_adapter = adapters
            .mcp_registry
            .map(OfficialRegistry::resolve_adapter)
            .transpose()
            .map_err(|error| PluginError::Unavailable(error.to_string()))?;
        Self::assemble(HostInitialization {
            data_directory: data_directory.into(),
            bridge: bridge.into(),
            providers,
            initial_provider,
            initial_model: initial_model.into(),
            credential_store: credential_store.into(),
            mcp_adapter,
            mcp_registry_adapter,
            global_skill_source: default_global_source(),
        })
    }

    fn assemble(initialization: HostInitialization) -> Result<Self, LocalHostError> {
        let HostInitialization {
            data_directory,
            bridge,
            providers,
            initial_provider,
            initial_model,
            credential_store,
            mcp_adapter,
            mcp_registry_adapter,
            global_skill_source,
        } = initialization;
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
        std::fs::create_dir_all(&data_directory)?;
        let data_directory = std::fs::canonicalize(data_directory)?;
        let sessions = data_directory.join("sessions");
        std::fs::create_dir_all(&sessions)?;
        let sessions = std::fs::canonicalize(sessions)?;
        let host_database = data_directory.join(catalog::HOST_DATABASE);
        catalog::initialize(&host_database)?;
        let mcp_catalog = McpCatalogStore::open(host_database.clone())?;
        let mcp_credentials = McpCredentialResolver::default();
        let skill_store = SkillStore::initialize(
            host_database.clone(),
            store_path(&data_directory),
            global_skill_source,
        )?;
        let plugins = PluginManager::initialize(
            host_database.clone(),
            data_directory.join(PLUGIN_STORE_DIRECTORY),
            mcp_catalog.clone(),
            mcp_adapter.clone(),
            mcp_registry_adapter,
            mcp_credentials.clone(),
            skill_store.clone(),
        )?;
        Ok(Self {
            config: Arc::new(HostConfig {
                sessions,
                bridge,
                providers,
                initial_provider,
                initial_model,
                credential_store,
                mcp_catalog,
                mcp_adapter,
                mcp_credentials,
                skill_store,
                plugins,
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
        let agent_id = AgentId::new();
        let session_uuid = Uuid::new_v4();
        let session_id = SessionId::from_uuid(session_uuid);
        self.resolve_runtime(session_id, None, model, reasoning, &workspace, None)
            .await?;
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
        tokio::task::spawn_blocking(move || delete_session_storage(&sessions, session_id))
            .await??;
        let skills = self.config.skill_store.clone();
        tokio::task::spawn_blocking(move || skills.remove_session(session_id)).await??;
        Ok(())
    }

    async fn models(&self) -> Result<Vec<ModelChoice>, LocalHostError> {
        discover_enabled_models(&self.config).await
    }

    async fn resolve_runtime(
        &self,
        session_id: SessionId,
        command_id: Option<CommandId>,
        model: &ModelChoice,
        reasoning: ReasoningLevel,
        workspace: &LocalWorkspace,
        events: Option<Arc<dyn AgentEventSink>>,
    ) -> Result<renoa_kernel::Runtime, LocalHostError> {
        resolve_runtime(
            &self.config,
            session_id,
            command_id,
            model,
            reasoning,
            workspace,
            events,
        )
        .await
    }
}

pub(crate) async fn resolve_runtime(
    host: &HostConfig,
    session_id: SessionId,
    command_id: Option<CommandId>,
    model: &ModelChoice,
    reasoning: ReasoningLevel,
    workspace: &LocalWorkspace,
    events: Option<Arc<dyn AgentEventSink>>,
) -> Result<renoa_kernel::Runtime, LocalHostError> {
    let mut extension_tools = alpha_registry_bindings(
        host.mcp_catalog.clone(),
        host.mcp_adapter.clone(),
        host.mcp_credentials.clone(),
        session_id,
        command_id,
    );
    extension_tools.push(alpha_plugin_binding(
        host.plugins.clone(),
        workspace.root().to_path_buf(),
        session_id,
        command_id,
    ));
    extension_tools.extend(alpha_skill_bindings(
        host.skill_store.clone(),
        workspace.root().to_path_buf(),
        session_id,
        command_id,
    ));
    let skills = host.skill_store.clone();
    let skill_context =
        tokio::task::spawn_blocking(move || runtime_context(&skills, session_id, command_id))
            .await??;
    let mut config = LocalRuntimeConfig::for_alpha(
        host.bridge.clone(),
        model.provider().as_str(),
        model.id(),
        host.credential_store.clone(),
        workspace,
    )?
    .with_discovered_model(model)
    .with_reasoning(reasoning);
    if let Some(skill_context) = skill_context {
        config = config.with_skill_context(skill_context);
    }
    Ok(build_composed_local_runtime(config, workspace, extension_tools, events).await?)
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
