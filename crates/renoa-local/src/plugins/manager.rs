use std::path::PathBuf;

use renoa_agent::ToolUpdates;
use tokio_util::sync::CancellationToken;

mod bootstrap;
mod connect;
mod credential;
mod identity;
mod preflight;
mod shared;
mod status;

use super::{
    InstalledPlugin, OfficialRegistry, PluginConnectionRequest, PluginCredential, PluginError,
    discovery::{RegistryError, RegistryLookupResult, RegistrySearchResult},
    store::PluginStore,
};
#[cfg(test)]
use crate::mcp::McpCredentialResolver;
use renoa_kernel::AgentId;

use crate::{
    mcp::{McpAuthorizationResolver, McpCatalogSnapshot, McpCatalogStore},
    shared_registry::SharedPluginRegistry,
    skills::{SkillComponentReport, SkillStore},
};
use identity::default_connection_id;
pub(super) use identity::integration_id;

pub(crate) use connect::{ProfileAuthorizationRequest, ProfileConnectionRequest};

#[derive(Clone)]
pub(crate) struct PluginManager {
    pub(super) store: PluginStore,
    mcp_catalog: McpCatalogStore,
    mcp_adapter: Option<PathBuf>,
    registry: Option<OfficialRegistry>,
    authorizations: McpAuthorizationResolver,
    skills: SkillStore,
    shared_registry: Option<SharedPluginRegistry>,
    pub(super) github_source: super::intake::GithubSourceClient,
}

impl PluginManager {
    pub(crate) fn mcp_catalog(&self) -> McpCatalogStore {
        self.mcp_catalog.clone()
    }

    #[cfg(test)]
    pub(crate) fn initialize(
        database: PathBuf,
        packages: PathBuf,
        mcp_catalog: McpCatalogStore,
        mcp_adapter: Option<PathBuf>,
        registry_adapter: Option<PathBuf>,
        credentials: McpCredentialResolver,
        skills: SkillStore,
    ) -> Result<Self, PluginError> {
        let authorizations =
            McpAuthorizationResolver::new(&mcp_catalog, mcp_adapter.clone(), credentials);
        Self::initialize_with_authorizations(
            database,
            packages,
            mcp_catalog,
            mcp_adapter,
            registry_adapter,
            authorizations,
            skills,
        )
    }

    pub(crate) fn initialize_with_authorizations(
        database: PathBuf,
        packages: PathBuf,
        mcp_catalog: McpCatalogStore,
        mcp_adapter: Option<PathBuf>,
        registry_adapter: Option<PathBuf>,
        authorizations: McpAuthorizationResolver,
        skills: SkillStore,
    ) -> Result<Self, PluginError> {
        Ok(Self {
            store: PluginStore::initialize(database, packages)?,
            mcp_catalog,
            mcp_adapter,
            registry: registry_adapter.map(OfficialRegistry::new),
            authorizations,
            skills,
            shared_registry: None,
            github_source: super::intake::GithubSourceClient::default(),
        })
    }

    pub(crate) fn with_shared_registry(
        mut self,
        shared_registry: Option<SharedPluginRegistry>,
    ) -> Self {
        self.shared_registry = shared_registry;
        self
    }

    pub(crate) async fn search_registry(
        &self,
        query: &str,
        cancellation: CancellationToken,
    ) -> Result<RegistrySearchResult, RegistryError> {
        let registry = self.registry.as_ref().ok_or_else(|| {
            RegistryError::Unavailable(
                "RENOA_MCP_REGISTRY_ADAPTER must be set before searching the official Registry"
                    .to_owned(),
            )
        })?;
        registry.search(query, cancellation).await
    }

    pub(crate) async fn lookup_registry(
        &self,
        registry_name: &str,
        registry_version: &str,
        cancellation: CancellationToken,
    ) -> Result<RegistryLookupResult, RegistryError> {
        let registry = self.registry.as_ref().ok_or_else(|| {
            RegistryError::Unavailable(
                "RENOA_MCP_REGISTRY_ADAPTER must be set before looking up an official Registry record"
                    .to_owned(),
            )
        })?;
        registry
            .lookup(registry_name, registry_version, cancellation)
            .await
    }

    pub(crate) async fn add_to_profile(
        &self,
        context: AddOperationContext<'_>,
        source: super::api::PluginSource,
        expected: Option<String>,
        connection_request: Option<PluginConnectionRequest>,
        cancellation: CancellationToken,
    ) -> Result<PluginAddOutcome, PluginError> {
        super::intake::require_active(&cancellation)?;
        if matches!(source, super::api::PluginSource::Installed { .. })
            && connection_request.is_some()
        {
            return Err(PluginError::Invalid("installed package reuse only enables skills; omit server, connection, credential, and replace, then use enable for an existing connection or connect for a new one".to_owned()));
        }
        let connect_by_default = matches!(source, super::api::PluginSource::Mcp { .. });
        let generated_server = match &source {
            super::api::PluginSource::Mcp { server, .. } => Some(server.clone()),
            _ => None,
        };
        let receipt = super::intake::receipt(&source);
        match &source {
            super::api::PluginSource::Installed {..} | super::api::PluginSource::Mcp {..} if expected.is_some() => return Err(PluginError::Invalid("installed and MCP sources do not accept expected_digest in add".to_owned())),
            super::api::PluginSource::Package {..} | super::api::PluginSource::Skill {..} | super::api::PluginSource::Github {..} if expected.is_none() => return Err(PluginError::Invalid("add requires expected_digest from inspect for package, skill, and GitHub sources".to_owned())),
            _ => (),
        }
        let existing = if let super::api::PluginSource::Installed { package_digest } = &source {
            Some(self.load_available(package_digest).await?)
        } else if let Some(expected) = &expected {
            super::store::validate_digest(expected)?;
            match self.load_local(expected).await {
                Ok(installed) => Some(installed),
                Err(PluginError::NotFound(_)) => None,
                Err(error) => return Err(error),
            }
        } else {
            None
        };
        let captured = if existing.is_none() {
            Some(
                self.capture_source(source, context.workspace, cancellation.clone())
                    .await?,
            )
        } else {
            None
        };
        let (digest, servers) = if let Some(existing) = &existing {
            (existing.digest(), existing.mcp_servers())
        } else {
            let captured = captured.as_ref().expect("new source was captured");
            (
                captured.inspection.digest(),
                captured.inspection.mcp_servers(),
            )
        };
        if let Some(expected) = &expected
            && expected != digest
        {
            return Err(PluginError::Conflict(format!(
                "plugin source changed after inspection: expected {expected}, found {digest}"
            )));
        }
        let catalog = self.mcp_catalog.clone();
        let digest = digest.to_owned();
        let servers = servers.to_vec();
        let preflight_request = connection_request.clone();
        let default_server = generated_server.clone();
        tokio::task::spawn_blocking(move || {
            preflight::add_connection(
                &catalog,
                &digest,
                &servers,
                preflight_request.as_ref(),
                connect_by_default,
                default_server.as_deref(),
            )
        })
        .await??;
        super::intake::require_active(&cancellation)?;
        let installed = if let Some(installed) = existing {
            installed
        } else {
            let store = self.store.clone();
            let captured = captured.expect("new source was captured");
            tokio::task::spawn_blocking(move || store.install_captured(&captured)).await??
        };
        self.synchronize_installed(&installed).await?;
        let skills = self.sync_skills(context.agent_id, &installed).await?;
        self.connect_prepared(
            PreparedExtension {
                installed,
                source: receipt,
                generated_server,
                connect_by_default,
            },
            skills,
            connection_request,
            context,
            cancellation,
        )
        .await
    }

    async fn sync_skills(
        &self,
        agent_id: &AgentId,
        installed: &InstalledPlugin,
    ) -> Result<SkillComponentReport, PluginError> {
        let store = self.store.clone();
        let package_digest = installed.digest().to_owned();
        let plugin_name = installed.metadata().name().to_owned();
        let agent_id = *agent_id;
        let skills = self.skills.clone();
        tokio::task::spawn_blocking(move || {
            let package_root = store.package_root(&package_digest)?;
            skills
                .sync_plugin(&agent_id.to_string(), &plugin_name, &package_root)
                .map_err(PluginError::from)
        })
        .await?
    }

    async fn connect_prepared(
        &self,
        prepared: PreparedExtension,
        skills: SkillComponentReport,
        request: Option<PluginConnectionRequest>,
        context: AddOperationContext<'_>,
        cancellation: CancellationToken,
    ) -> Result<PluginAddOutcome, PluginError> {
        if request.is_none() && !prepared.connect_by_default {
            return Ok(PluginAddOutcome {
                installed: prepared.installed,
                source: prepared.source,
                skills,
                connection: PluginConnectionOutcome::NotRequested,
            });
        }
        let PluginConnectionRequest {
            id,
            server,
            credential,
            replace,
        } = request.unwrap_or_else(|| {
            PluginConnectionRequest::new(None, None, PluginCredential::None, false)
        });
        let server = match server.or(prepared.generated_server) {
            Some(server) => server,
            None if prepared.installed.mcp_servers().len() == 1 => {
                prepared.installed.mcp_servers()[0].id().to_owned()
            }
            None => {
                return Ok(PluginAddOutcome {
                    installed: prepared.installed,
                    source: prepared.source,
                    skills,
                    connection: PluginConnectionOutcome::Failed {
                        id,
                        server: None,
                        error: PluginError::Invalid(
                            "adding this package with a connection requires an exact MCP server id"
                                .to_owned(),
                        ),
                    },
                });
            }
        };
        let connection =
            id.unwrap_or_else(|| default_connection_id(prepared.installed.digest(), &server));
        let outcome = match self
            .connect_profile_operation(
                ProfileConnectionRequest {
                    agent_id: context.agent_id,
                    package_digest: prepared.installed.digest(),
                    server_id: &server,
                    connection_id: &connection,
                    credential,
                    replace,
                    restart: false,
                    requested_scope: None,
                    operation_id: context.operation_id,
                    updates: context.updates,
                },
                cancellation,
            )
            .await
        {
            Ok(snapshot) => PluginConnectionOutcome::Connected {
                id: connection,
                server,
                snapshot,
            },
            Err(error) => PluginConnectionOutcome::Failed {
                id: Some(connection),
                server: Some(server),
                error,
            },
        };
        Ok(PluginAddOutcome {
            installed: prepared.installed,
            source: prepared.source,
            skills,
            connection: outcome,
        })
    }
}

pub(crate) struct AddOperationContext<'a> {
    pub(crate) workspace: &'a std::path::Path,
    pub(crate) agent_id: &'a AgentId,
    pub(crate) operation_id: &'a str,
    pub(crate) updates: Option<&'a ToolUpdates>,
}

struct PreparedExtension {
    installed: InstalledPlugin,
    source: PluginSourceReceipt,
    generated_server: Option<String>,
    connect_by_default: bool,
}

pub struct PluginAddOutcome {
    pub installed: InstalledPlugin,
    pub source: PluginSourceReceipt,
    pub skills: SkillComponentReport,
    pub connection: PluginConnectionOutcome,
}

pub enum PluginSourceReceipt {
    Mcp,
    Package,
    Installed,
    Skill,
    Github,
}

pub enum PluginConnectionOutcome {
    NotRequested,
    Connected {
        id: String,
        server: String,
        snapshot: McpCatalogSnapshot,
    },
    Failed {
        id: Option<String>,
        server: Option<String>,
        error: PluginError,
    },
}
