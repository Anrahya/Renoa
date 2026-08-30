use std::{
    collections::{BTreeMap, HashSet},
    path::{Path, PathBuf},
    sync::Arc,
};

use renoa_kernel::{AgentId, SessionId};
use thiserror::Error;
use uuid::Uuid;

pub(crate) mod catalog;
mod extensions;
mod mcp;
mod models;
mod profiles;
mod runtime;
#[cfg(test)]
mod skill_tests;

use crate::agent_session::AgentSessionStorage;
use crate::{
    AgentProfile, AgentProfileError, AgentProfileId, AgentSession, LocalRuntimeError, LocalSession,
    LocalSessionError, LocalWorkspace, LocalWorkspaceError, ModelBridgeError, ModelChoice,
    ModelProvider,
    host_storage::{
        KERNEL_DATABASE, MANIFEST_FILE, create_session_storage, delete_session_storage,
        read_manifest,
    },
    mcp::{McpCatalogStore, McpCredentialResolver, McpHostError, resolve_adapter},
    plugins::{OfficialRegistry, PLUGIN_STORE_DIRECTORY, PluginError, PluginManager},
    selection::{RuntimeSelection, SELECTION_FILE, read_selection},
    skills::{SkillError, SkillStore, default_global_source, store_path},
    trace::{TRACE_DATABASE, TraceError, TraceStore},
};

use models::validate_selection;
pub(crate) use models::{
    discover_enabled_models, initial_reasoning, require_model, selected_model_by_selection_id,
};
use profiles::collect_profiles;
pub(crate) use runtime::{RuntimeRequest, resolve_runtime};

/// Process-local configuration used to assemble Renoa Agent sessions.
pub struct LocalHost {
    config: Arc<HostConfig>,
}

/// Optional replaceable process adapters used by the local Host.
#[derive(Clone, Copy, Default)]
pub struct LocalHostAdapters<'a> {
    mcp: Option<&'a Path>,
    mcp_registry: Option<&'a Path>,
    shared_plugin_registry: Option<&'a str>,
}

impl<'a> LocalHostAdapters<'a> {
    /// Selects the MCP runtime adapter.
    #[must_use]
    pub const fn new(mcp: Option<&'a Path>) -> Self {
        Self {
            mcp,
            mcp_registry: None,
            shared_plugin_registry: None,
        }
    }

    /// Selects the official MCP Registry discovery adapter.
    #[must_use]
    pub const fn with_mcp_registry(mut self, registry: Option<&'a Path>) -> Self {
        self.mcp_registry = registry;
        self
    }

    /// Selects a private shared Agent Plugin registry origin.
    #[must_use]
    pub const fn with_shared_plugin_registry(mut self, registry: Option<&'a str>) -> Self {
        self.shared_plugin_registry = registry;
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
    pub(crate) profiles: BTreeMap<AgentProfileId, AgentProfile>,
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
    shared_plugin_registry: Option<String>,
    global_skill_source: Option<PathBuf>,
    profiles: Vec<AgentProfile>,
}

/// Model-provider settings shared by every profile assembled by one Host.
pub struct LocalModelConfiguration {
    bridge: PathBuf,
    providers: Vec<ModelProvider>,
    initial_provider: ModelProvider,
    initial_model: String,
    credential_store: PathBuf,
}

impl LocalModelConfiguration {
    #[must_use]
    pub fn new(
        bridge: impl Into<PathBuf>,
        providers: Vec<ModelProvider>,
        initial_provider: ModelProvider,
        initial_model: impl Into<String>,
        credential_store: impl Into<PathBuf>,
    ) -> Self {
        Self {
            bridge: bridge.into(),
            providers,
            initial_provider,
            initial_model: initial_model.into(),
            credential_store: credential_store.into(),
        }
    }
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
    Profile(#[from] AgentProfileError),
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
        models: LocalModelConfiguration,
        profiles: Vec<AgentProfile>,
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
            bridge: models.bridge,
            providers: models.providers,
            initial_provider: models.initial_provider,
            initial_model: models.initial_model,
            credential_store: models.credential_store,
            mcp_adapter,
            mcp_registry_adapter,
            shared_plugin_registry: adapters.shared_plugin_registry.map(str::to_owned),
            global_skill_source: default_global_source(),
            profiles,
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
            shared_plugin_registry,
            global_skill_source,
            profiles,
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
        let profiles = collect_profiles(profiles)?;
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
        let shared_plugin_registry = shared_plugin_registry
            .map(|endpoint| {
                crate::shared_registry::SharedPluginRegistry::new(
                    &endpoint,
                    host_database.clone(),
                    &data_directory,
                )
            })
            .transpose()
            .map_err(|error| LocalHostError::Configuration(error.to_string()))?;
        let plugins = PluginManager::initialize(
            host_database.clone(),
            data_directory.join(PLUGIN_STORE_DIRECTORY),
            mcp_catalog.clone(),
            mcp_adapter.clone(),
            mcp_registry_adapter,
            mcp_credentials.clone(),
            skill_store.clone(),
        )?
        .with_shared_registry(shared_plugin_registry);
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
                profiles,
            }),
        })
    }

    /// Returns every profile currently available to new Agent instances.
    #[must_use]
    pub fn profile_ids(&self) -> Vec<AgentProfileId> {
        self.config.profiles.keys().cloned().collect()
    }

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
        require_absolute(cwd)?;
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
        let session_uuid = Uuid::new_v4();
        let session_id = SessionId::from_uuid(session_uuid);
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
        let directory = tokio::task::spawn_blocking(move || {
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
        let kernel = LocalSession::load(directory.join(KERNEL_DATABASE), session_id)?;
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

    async fn models(&self) -> Result<Vec<ModelChoice>, LocalHostError> {
        discover_enabled_models(&self.config).await
    }

    fn profile(&self, profile_id: &AgentProfileId) -> Result<&AgentProfile, LocalHostError> {
        self.config.profiles.get(profile_id).ok_or_else(|| {
            LocalHostError::InvalidRequest(format!(
                "agent profile `{profile_id}` is not registered with this Host"
            ))
        })
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
