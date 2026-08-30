use std::path::PathBuf;

use renoa_agent::ToolUpdates;
use tokio_util::sync::CancellationToken;

mod credential;
mod identity;
mod status;

use super::{
    ExtensionAddRequest, ExtensionConnectionRequest, ExtensionSource, InstalledPlugin,
    OfficialRegistry, PluginCredential, PluginError, PluginInspection,
    discovery::{RegistryError, RegistryLookupResult, RegistrySearchResult},
    generated::GeneratedMcpPlugin,
    store::PluginStore,
};
use crate::{
    AgentProfileId,
    mcp::{
        McpAdapterError, McpAuthorizationResolver, McpCatalogSnapshot, McpCatalogStore,
        McpCredentialResolver, McpHostError, McpOAuthAuthorizationRequest, discover_cancellable,
    },
    skills::{SkillComponentReport, SkillStore},
};
use credential::credential_auth;
use identity::{default_connection_id, integration_id};

#[derive(Clone)]
pub(crate) struct PluginManager {
    store: PluginStore,
    mcp_catalog: McpCatalogStore,
    mcp_adapter: Option<PathBuf>,
    registry: Option<OfficialRegistry>,
    authorizations: McpAuthorizationResolver,
    skills: SkillStore,
}

pub(crate) struct ProfileConnectionRequest<'a> {
    pub(crate) profile_id: &'a AgentProfileId,
    pub(crate) package_digest: &'a str,
    pub(crate) server_id: &'a str,
    pub(crate) connection_id: &'a str,
    pub(crate) credential: PluginCredential,
    pub(crate) replace: bool,
    pub(crate) operation_id: &'a str,
}

impl PluginManager {
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
        Ok(Self {
            store: PluginStore::initialize(database, packages)?,
            mcp_catalog,
            mcp_adapter,
            registry: registry_adapter.map(OfficialRegistry::new),
            authorizations,
            skills,
        })
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

    pub(crate) async fn inspect(
        &self,
        source: impl Into<PathBuf>,
    ) -> Result<PluginInspection, PluginError> {
        let source = source.into();
        tokio::task::spawn_blocking(move || {
            super::inspect::inspect(&source).map(|item| item.inspection)
        })
        .await?
    }

    pub(crate) async fn install(
        &self,
        source: impl Into<PathBuf>,
        expected_digest: impl Into<String>,
    ) -> Result<InstalledPlugin, PluginError> {
        let source = source.into();
        let expected_digest = expected_digest.into();
        let store = self.store.clone();
        tokio::task::spawn_blocking(move || store.install(&source, &expected_digest)).await?
    }

    pub(crate) async fn list(&self) -> Result<Vec<InstalledPlugin>, PluginError> {
        let store = self.store.clone();
        tokio::task::spawn_blocking(move || store.list()).await?
    }

    pub(crate) async fn list_report(&self) -> Result<super::PluginListReport, PluginError> {
        let store = self.store.clone();
        tokio::task::spawn_blocking(move || store.list_report()).await?
    }

    pub(crate) async fn add_to_profile(
        &self,
        profile_id: &AgentProfileId,
        request: ExtensionAddRequest,
        operation_id: &str,
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
            profile_id,
            prepared,
            skills,
            connection_request,
            operation_id,
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
        profile_id: &AgentProfileId,
        prepared: PreparedExtension,
        skills: SkillComponentReport,
        request: Option<ExtensionConnectionRequest>,
        operation_id: &str,
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
                    profile_id,
                    package_digest: prepared.installed.digest(),
                    server_id: &server,
                    connection_id: &connection,
                    credential,
                    replace,
                    operation_id,
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

    pub(crate) async fn connect_profile(
        &self,
        profile_id: &AgentProfileId,
        package_digest: &str,
        server_id: &str,
        connection_id: &str,
        credential: PluginCredential,
        cancellation: CancellationToken,
    ) -> Result<McpCatalogSnapshot, PluginError> {
        let operation_id = format!("host-connect.{}", uuid::Uuid::new_v4());
        self.connect_profile_operation(
            ProfileConnectionRequest {
                profile_id,
                package_digest,
                server_id,
                connection_id,
                credential,
                replace: false,
                operation_id: &operation_id,
            },
            cancellation,
        )
        .await
    }

    pub(crate) async fn connect_profile_operation(
        &self,
        request: ProfileConnectionRequest<'_>,
        cancellation: CancellationToken,
    ) -> Result<McpCatalogSnapshot, PluginError> {
        let ProfileConnectionRequest {
            profile_id,
            package_digest,
            server_id,
            connection_id,
            credential,
            replace,
            operation_id,
        } = request;
        let store = self.store.clone();
        let digest = package_digest.to_owned();
        let plugin = tokio::task::spawn_blocking(move || store.load(&digest)).await??;
        let server = plugin
            .mcp_servers()
            .iter()
            .find(|server| server.id() == server_id)
            .cloned()
            .ok_or_else(|| {
                PluginError::NotFound(format!(
                    "package '{}' has no supported MCP server '{}'",
                    plugin.digest(),
                    server_id,
                ))
            })?;
        let adapter = self.mcp_adapter.clone().ok_or_else(|| {
            PluginError::Unavailable(
                "RENOA_MCP_ADAPTER must be set before connecting a package MCP server".to_owned(),
            )
        })?;
        let auth = credential_auth(credential, connection_id, server.endpoint())?;
        let integration_id = integration_id(plugin.digest(), server.id());
        let registered_headers = crate::mcp::McpRequestHeaders::new(
            server
                .request_headers()
                .iter()
                .map(|(name, value)| (name.clone(), value.clone())),
        )?;
        let catalog = self.mcp_catalog.clone();
        let registered_integration = integration_id.clone();
        let registered_connection = connection_id.to_owned();
        let registered_endpoint = server.endpoint().to_owned();
        let registered_headers_copy = registered_headers.clone();
        let registered_auth = auth.clone();
        tokio::task::spawn_blocking(move || {
            if replace {
                catalog.replace_connection(
                    &registered_integration,
                    &registered_connection,
                    &registered_endpoint,
                    &registered_headers_copy,
                    &registered_auth,
                )
            } else {
                catalog.register_connection(
                    &registered_integration,
                    &registered_connection,
                    &registered_endpoint,
                    &registered_headers_copy,
                    &registered_auth,
                )
            }
        })
        .await??;
        let authorization = self
            .authorizations
            .resolve(
                connection_id,
                server.endpoint(),
                &auth,
                operation_id,
                cancellation.clone(),
            )
            .await?;
        if cancellation.is_cancelled() {
            return Err(McpHostError::from(McpAdapterError::Cancelled).into());
        }
        let snapshot = discover_cancellable(
            &adapter,
            connection_id,
            server.endpoint(),
            &registered_headers,
            authorization.as_ref(),
            cancellation,
        )
        .await?;
        let catalog = self.mcp_catalog.clone();
        let enabled_profile = profile_id.clone();
        let stored_snapshot = snapshot.clone();
        tokio::task::spawn_blocking(move || {
            catalog.publish_and_enable_connection(enabled_profile.as_str(), &stored_snapshot)
        })
        .await??;
        Ok(snapshot)
    }

    pub(crate) async fn authorize_profile(
        &self,
        profile_id: &AgentProfileId,
        connection_id: &str,
        operation_id: &str,
        restart: bool,
        updates: Option<&ToolUpdates>,
        cancellation: CancellationToken,
    ) -> Result<McpCatalogSnapshot, PluginError> {
        let catalog = self.mcp_catalog.clone();
        let stored_connection = connection_id.to_owned();
        let connection =
            tokio::task::spawn_blocking(move || catalog.connection_config(&stored_connection))
                .await??;
        let authorization = self
            .authorizations
            .authorize(
                McpOAuthAuthorizationRequest {
                    connection_id,
                    endpoint: &connection.endpoint,
                    reference: &connection.auth,
                    operation_id,
                    restart,
                    updates,
                },
                cancellation.clone(),
            )
            .await?;
        let adapter = self.mcp_adapter.as_deref().ok_or_else(|| {
            PluginError::Unavailable(
                "RENOA_MCP_ADAPTER must be set before authorizing a package MCP server".to_owned(),
            )
        })?;
        let snapshot = discover_cancellable(
            adapter,
            connection_id,
            &connection.endpoint,
            &connection.request_headers,
            Some(&authorization),
            cancellation,
        )
        .await?;
        let catalog = self.mcp_catalog.clone();
        let enabled_profile = profile_id.clone();
        let stored_snapshot = snapshot.clone();
        tokio::task::spawn_blocking(move || {
            catalog.publish_and_enable_connection(enabled_profile.as_str(), &stored_snapshot)
        })
        .await??;
        Ok(snapshot)
    }
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
