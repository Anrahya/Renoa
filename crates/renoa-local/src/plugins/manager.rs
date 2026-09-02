use std::path::PathBuf;

use renoa_agent::ToolUpdates;
use tokio_util::sync::CancellationToken;

mod connect;
mod credential;
mod identity;
mod shared;
mod status;

use super::{
    ExtensionAddRequest, ExtensionConnectionRequest, ExtensionSource, InstalledPlugin,
    OfficialRegistry, PluginCredential, PluginError,
    discovery::{RegistryError, RegistryLookupResult, RegistrySearchResult},
    generated::GeneratedMcpPlugin,
    store::PluginStore,
};
#[cfg(test)]
use crate::mcp::McpCredentialResolver;
use crate::{
    AgentProfileId,
    mcp::{McpAuthorizationResolver, McpCatalogSnapshot, McpCatalogStore},
    shared_registry::SharedPluginRegistry,
    skills::{SkillComponentReport, SkillStore},
};
use identity::default_connection_id;

pub(crate) use connect::ProfileConnectionRequest;

#[derive(Clone)]
pub(crate) struct PluginManager {
    store: PluginStore,
    mcp_catalog: McpCatalogStore,
    mcp_adapter: Option<PathBuf>,
    registry: Option<OfficialRegistry>,
    authorizations: McpAuthorizationResolver,
    skills: SkillStore,
    shared_registry: Option<SharedPluginRegistry>,
}

impl PluginManager {
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
        profile_id: &AgentProfileId,
        request: ExtensionAddRequest,
        operation_id: &str,
        updates: Option<&ToolUpdates>,
        cancellation: CancellationToken,
    ) -> Result<ExtensionAddOutcome, PluginError> {
        let connection_request = request.connection;
        let prepared = match request.source {
            ExtensionSource::Mcp(source) => {
                let generated = GeneratedMcpPlugin::from_researched(source)?;
                let server = generated.server().to_owned();
                let store = self.store.clone();
                let installed =
                    tokio::task::spawn_blocking(move || store.install_generated(&generated))
                        .await??;
                self.synchronize_installed(&installed).await?;
                PreparedExtension {
                    installed,
                    source: ExtensionSourceReceipt::Mcp,
                    generated_server: Some(server),
                    connect_by_default: true,
                }
            }
            ExtensionSource::Package {
                path,
                expected_digest,
            } => {
                let installed = self.install(path, expected_digest).await?;
                PreparedExtension {
                    installed,
                    source: ExtensionSourceReceipt::Package,
                    generated_server: None,
                    connect_by_default: false,
                }
            }
        };
        let skills = self.sync_skills(profile_id, &prepared.installed).await?;
        self.connect_prepared(
            prepared,
            skills,
            connection_request,
            AddOperationContext {
                profile_id,
                operation_id,
                updates,
            },
            cancellation,
        )
        .await
    }

    async fn sync_skills(
        &self,
        profile_id: &AgentProfileId,
        installed: &InstalledPlugin,
    ) -> Result<SkillComponentReport, PluginError> {
        let store = self.store.clone();
        let package_digest = installed.digest().to_owned();
        let plugin_name = installed.metadata().name().to_owned();
        let profile_id = profile_id.clone();
        let skills = self.skills.clone();
        tokio::task::spawn_blocking(move || {
            let package_root = store.package_root(&package_digest)?;
            skills
                .sync_plugin(profile_id.as_str(), &plugin_name, &package_root)
                .map_err(PluginError::from)
        })
        .await?
    }

    async fn connect_prepared(
        &self,
        prepared: PreparedExtension,
        skills: SkillComponentReport,
        request: Option<ExtensionConnectionRequest>,
        context: AddOperationContext<'_>,
        cancellation: CancellationToken,
    ) -> Result<ExtensionAddOutcome, PluginError> {
        if request.is_none() && !prepared.connect_by_default {
            return Ok(ExtensionAddOutcome {
                installed: prepared.installed,
                source: prepared.source,
                skills,
                connection: ExtensionConnectionOutcome::NotRequested,
            });
        }
        let ExtensionConnectionRequest {
            id,
            server,
            credential,
            replace,
        } = request.unwrap_or_else(|| {
            ExtensionConnectionRequest::new(None, None, PluginCredential::None, false)
        });
        let server = match server.or(prepared.generated_server) {
            Some(server) => server,
            None if prepared.installed.mcp_servers().len() == 1 => {
                prepared.installed.mcp_servers()[0].id().to_owned()
            }
            None => {
                return Ok(ExtensionAddOutcome {
                    installed: prepared.installed,
                    source: prepared.source,
                    skills,
                    connection: ExtensionConnectionOutcome::Failed {
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
                    profile_id: context.profile_id,
                    package_digest: prepared.installed.digest(),
                    server_id: &server,
                    connection_id: &connection,
                    credential,
                    replace,
                    operation_id: context.operation_id,
                    updates: context.updates,
                },
                cancellation,
            )
            .await
        {
            Ok(snapshot) => ExtensionConnectionOutcome::Connected {
                id: connection,
                server,
                snapshot,
            },
            Err(error) => ExtensionConnectionOutcome::Failed {
                id: Some(connection),
                server: Some(server),
                error,
            },
        };
        Ok(ExtensionAddOutcome {
            installed: prepared.installed,
            source: prepared.source,
            skills,
            connection: outcome,
        })
    }
}

struct AddOperationContext<'a> {
    profile_id: &'a AgentProfileId,
    operation_id: &'a str,
    updates: Option<&'a ToolUpdates>,
}

struct PreparedExtension {
    installed: InstalledPlugin,
    source: ExtensionSourceReceipt,
    generated_server: Option<String>,
    connect_by_default: bool,
}

pub(crate) struct ExtensionAddOutcome {
    pub(crate) installed: InstalledPlugin,
    pub(crate) source: ExtensionSourceReceipt,
    pub(crate) skills: SkillComponentReport,
    pub(crate) connection: ExtensionConnectionOutcome,
}

pub(crate) enum ExtensionSourceReceipt {
    Mcp,
    Package,
}

pub(crate) enum ExtensionConnectionOutcome {
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
