use std::{
    collections::HashSet,
    path::{Path, PathBuf},
    sync::Arc,
};

use thiserror::Error;

pub(crate) mod agents;
pub(crate) mod catalog;
pub(crate) mod definition;
mod extensions;
pub(crate) mod history;
mod lease;
mod mcp;
mod models;
pub(crate) mod observation;
mod reset;
pub(crate) mod reviews;
pub(crate) mod routines;
mod runtime;
mod sessions;
#[cfg(all(test, unix))]
mod shared_capabilities_tests;
#[cfg(test)]
mod skill_tests;

use crate::{
    LocalRuntimeError, LocalSessionError, LocalWorkspaceError, ModelBridgeError, ModelProvider,
    ReasoningLevel,
    code_mode::MontyEvaluator,
    mcp::{
        McpAuthorizationResolver, McpCatalogStore, McpCredentialResolver, McpHostError,
        resolve_adapter,
    },
    plugins::{OfficialRegistry, PLUGIN_STORE_DIRECTORY, PluginError, PluginManager},
    skills::{SkillError, SkillStore, default_global_source, store_path},
    trace::TraceError,
};

pub(crate) use models::{
    discover_models_for, initial_reasoning, require_model, selected_model_by_selection_id,
};
pub use reset::{HostResetReport, reset_host_data_root};
pub(crate) use runtime::{RuntimeRequest, resolve_runtime};

/// Process-local configuration used to assemble Renoa Agent sessions.
#[derive(Clone)]
pub struct LocalHost {
    config: Arc<HostConfig>,
}

/// The one directory under the data root that holds Host session state.
const SESSIONS_DIRECTORY: &str = "sessions";

/// Optional replaceable process adapters used by the local Host.
#[derive(Clone, Copy, Default)]
pub struct LocalHostAdapters<'a> {
    mcp: Option<&'a Path>,
    mcp_registry: Option<&'a Path>,
    shared_plugin_registry: Option<&'a str>,
    oauth_relay: Option<(&'a str, &'a Path)>,
    code_mode_worker: Option<&'a Path>,
}

impl<'a> LocalHostAdapters<'a> {
    /// Selects the MCP runtime adapter.
    #[must_use]
    pub const fn new(mcp: Option<&'a Path>) -> Self {
        Self {
            mcp,
            mcp_registry: None,
            shared_plugin_registry: None,
            oauth_relay: None,
            code_mode_worker: None,
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

    /// Uses the self-hosted callback relay and private headless OAuth store.
    #[must_use]
    pub const fn with_oauth_relay(mut self, origin: &'a str, credentials: &'a Path) -> Self {
        self.oauth_relay = Some((origin, credentials));
        self
    }

    /// Selects the exact-pinned Monty worker for MCP-only Code Mode.
    #[must_use]
    pub const fn with_code_mode_worker(mut self, worker: Option<&'a Path>) -> Self {
        self.code_mode_worker = worker;
        self
    }
}

pub(crate) struct HostConfig {
    pub(crate) database: PathBuf,
    pub(crate) sessions: PathBuf,
    pub(crate) bridge: PathBuf,
    pub(crate) providers: Vec<ModelProvider>,
    pub(crate) initial_provider: ModelProvider,
    pub(crate) initial_model: String,
    pub(crate) initial_reasoning: Option<ReasoningLevel>,
    pub(crate) credential_store: PathBuf,
    pub(crate) mcp_catalog: McpCatalogStore,
    pub(crate) mcp_adapter: Option<PathBuf>,
    pub(crate) mcp_authorizations: McpAuthorizationResolver,
    pub(crate) skill_store: SkillStore,
    pub(crate) plugins: PluginManager,
    pub(crate) code_mode: Option<Arc<MontyEvaluator>>,
}

struct HostInitialization {
    data_directory: PathBuf,
    bridge: PathBuf,
    providers: Vec<ModelProvider>,
    initial_provider: ModelProvider,
    initial_model: String,
    initial_reasoning: Option<ReasoningLevel>,
    credential_store: PathBuf,
    mcp_adapter: Option<PathBuf>,
    mcp_registry_adapter: Option<PathBuf>,
    shared_plugin_registry: Option<String>,
    global_skill_source: Option<PathBuf>,
    oauth_relay: Option<(String, PathBuf)>,
    code_mode: Option<Arc<MontyEvaluator>>,
}

/// Model-provider settings shared by every agent assembled by one Host.
pub struct LocalModelConfiguration {
    bridge: PathBuf,
    providers: Vec<ModelProvider>,
    initial_provider: ModelProvider,
    initial_model: String,
    initial_reasoning: Option<ReasoningLevel>,
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
            initial_reasoning: None,
            credential_store: credential_store.into(),
        }
    }

    /// Selects the reasoning level used when a Host creates a new session.
    #[must_use]
    pub const fn with_initial_reasoning(mut self, reasoning: ReasoningLevel) -> Self {
        self.initial_reasoning = Some(reasoning);
        self
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
    Session(#[from] LocalSessionError),
    #[error(transparent)]
    TurnObservation(#[from] crate::TurnObservationError),
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
    #[error("agent {0} is already bound to different creation fields")]
    AgentConflict(renoa_kernel::AgentId),
    #[error("agent {0} is not registered with this Host")]
    AgentNotFound(renoa_kernel::AgentId),
    #[error("agent mutation cancelled before commit")]
    AgentCancelled,
    #[error(transparent)]
    Definition(#[from] crate::AgentDefinitionError),
    #[error(transparent)]
    Routine(#[from] routines::RoutineError),
    #[error(transparent)]
    GitHubReview(#[from] reviews::GitHubReviewError),
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
        adapters: LocalHostAdapters<'_>,
    ) -> Result<Self, LocalHostError> {
        let code_mode = adapters
            .code_mode_worker
            .map(MontyEvaluator::new)
            .transpose()
            .map_err(LocalHostError::Configuration)?
            .map(Arc::new);
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
            initial_reasoning: models.initial_reasoning,
            credential_store: models.credential_store,
            mcp_adapter,
            mcp_registry_adapter,
            shared_plugin_registry: adapters.shared_plugin_registry.map(str::to_owned),
            global_skill_source: default_global_source(),
            oauth_relay: adapters
                .oauth_relay
                .map(|(origin, credentials)| (origin.to_owned(), credentials.to_path_buf())),
            code_mode,
        })
    }

    fn assemble(initialization: HostInitialization) -> Result<Self, LocalHostError> {
        let HostInitialization {
            data_directory,
            bridge,
            providers,
            initial_provider,
            initial_model,
            initial_reasoning,
            credential_store,
            mcp_adapter,
            mcp_registry_adapter,
            shared_plugin_registry,
            global_skill_source,
            oauth_relay,
            code_mode,
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
        let sessions = session_root(&data_directory)?;
        let host_database = data_directory.join(catalog::HOST_DATABASE);
        catalog::initialize(&host_database)?;
        let mcp_catalog = McpCatalogStore::open(host_database.clone())?;
        let mcp_credentials = McpCredentialResolver::default();
        let mcp_authorizations = match oauth_relay {
            Some((origin, relay_credentials)) => McpAuthorizationResolver::with_remote_oauth(
                &mcp_catalog,
                mcp_adapter.clone(),
                mcp_credentials,
                &origin,
                &relay_credentials,
            )?,
            None => {
                McpAuthorizationResolver::new(&mcp_catalog, mcp_adapter.clone(), mcp_credentials)
            }
        };
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
        let plugins = PluginManager::initialize_with_authorizations(
            host_database.clone(),
            data_directory.join(PLUGIN_STORE_DIRECTORY),
            mcp_catalog.clone(),
            mcp_adapter.clone(),
            mcp_registry_adapter,
            mcp_authorizations.clone(),
            skill_store.clone(),
        )?
        .with_shared_registry(shared_plugin_registry);
        Ok(Self {
            config: Arc::new(HostConfig {
                database: host_database,
                sessions,
                bridge,
                providers,
                initial_provider,
                initial_model,
                initial_reasoning,
                credential_store,
                mcp_catalog,
                mcp_adapter,
                mcp_authorizations,
                skill_store,
                plugins,
                code_mode,
            }),
        })
    }
}

/// Creates or adopts `<data root>/sessions` and returns its canonical path.
///
/// The configured session root decides where agent documents and sessions are
/// stored, so it must be that exact child of the canonical data root. The path
/// is inspected without following a link before anything is created, and the
/// canonical result is compared with the path again, so a symbolic link or any
/// other file in its place is refused instead of adopted.
fn session_root(data_directory: &Path) -> Result<PathBuf, LocalHostError> {
    let sessions = data_directory.join(SESSIONS_DIRECTORY);
    match std::fs::symlink_metadata(&sessions) {
        Ok(metadata) if metadata.is_dir() => {}
        Ok(_) => return Err(unusable_session_root(&sessions)),
        Err(source) if source.kind() == std::io::ErrorKind::NotFound => {
            std::fs::create_dir(&sessions)?;
        }
        Err(source) => return Err(source.into()),
    }
    let resolved = std::fs::canonicalize(&sessions)?;
    if resolved != sessions {
        return Err(unusable_session_root(&resolved));
    }
    Ok(resolved)
}

fn unusable_session_root(path: &Path) -> LocalHostError {
    LocalHostError::Configuration(format!(
        "Host {SESSIONS_DIRECTORY} root `{}` must be the plain `{SESSIONS_DIRECTORY}` directory under the canonical data root, never a symbolic link or another file",
        path.display()
    ))
}

#[cfg(test)]
mod tests {
    use std::{fs, path::Path};

    use super::{HostInitialization, LocalHost, LocalHostError};
    use crate::ModelProvider;

    /// One Host data root whose `sessions` root is a symbolic link to a
    /// directory in a separate tree, which is the escape this module must
    /// refuse.
    fn a_symlinked_sessions_root() -> (tempfile::TempDir, tempfile::TempDir) {
        let directory = tempfile::tempdir().expect("fixture");
        let elsewhere = tempfile::tempdir().expect("escape target");
        let root = directory.path();
        fs::write(root.join("model.mjs"), "// fixture\n").expect("model");
        fs::write(root.join("auth.sqlite"), "").expect("auth boundary");
        fs::create_dir(root.join("data")).expect("data root");
        let link_target = elsewhere.path().join("linked-sessions");
        fs::create_dir(&link_target).expect("link target");
        std::os::unix::fs::symlink(&link_target, root.join("data/sessions"))
            .expect("link sessions");
        (directory, elsewhere)
    }

    fn assemble(root: &Path) -> Result<LocalHost, LocalHostError> {
        LocalHost::assemble(HostInitialization {
            data_directory: root.join("data"),
            bridge: root.join("model.mjs"),
            providers: vec![ModelProvider::Xai],
            initial_provider: ModelProvider::Xai,
            initial_model: "fixture".to_owned(),
            initial_reasoning: None,
            credential_store: root.join("auth.sqlite"),
            mcp_adapter: None,
            mcp_registry_adapter: None,
            shared_plugin_registry: None,
            global_skill_source: None,
            oauth_relay: None,
            code_mode: None,
        })
    }

    /// A `sessions` root that is a symbolic link would make the configured
    /// session root the link target, so assembly refuses it and creates nothing
    /// through the link.
    #[test]
    fn a_symlinked_sessions_root_is_refused() {
        let (directory, elsewhere) = a_symlinked_sessions_root();
        let Err(error) = assemble(directory.path()) else {
            panic!("a symlinked sessions root is refused");
        };
        assert!(
            matches!(&error, LocalHostError::Configuration(message) if message.contains("sessions root")),
            "unexpected error: {error}"
        );
        assert!(
            fs::read_dir(elsewhere.path().join("linked-sessions"))
                .expect("link target")
                .next()
                .is_none(),
            "assembly must not write through the link"
        );
        assert!(
            !directory.path().join("data/host.sqlite3").exists(),
            "a refused assembly must leave no catalog behind"
        );
    }

    /// The document root of every created agent is the sessions root's parent,
    /// so a `sessions` link that assembly accepted would publish documents
    /// under the link target instead of refusing the Host.
    #[tokio::test]
    async fn a_symlinked_sessions_root_publishes_no_agent_documents_outside_the_data_root() {
        let (directory, elsewhere) = a_symlinked_sessions_root();
        let Err(error) = assemble(directory.path()) else {
            panic!("a symlinked sessions root is refused before it can host an agent");
        };
        assert!(
            error.to_string().contains("sessions root"),
            "unexpected error: {error}"
        );
        assert!(
            !elsewhere.path().join("agents").exists(),
            "no agent document root may exist outside the Host data root"
        );
    }
}
