use std::path::PathBuf;

use renoa_agent::ToolUpdates;
use sha2::{Digest as _, Sha256};
use tokio_util::sync::CancellationToken;

use super::{
    CatalogCandidate, ExtensionAddRequest, ExtensionConnectionRequest, ExtensionSource,
    InstalledPlugin, IntegrationCatalog, PluginCredential, PluginError, PluginInspection,
    generated::GeneratedMcpPlugin, store::PluginStore,
};
use crate::{
    ALPHA_PROFILE_ID,
    mcp::{
        McpAdapterError, McpAuthorizationResolver, McpCatalogSnapshot, McpCatalogStore,
        McpConnectionAuth, McpCredentialResolver, McpHostError, McpOAuthAuthorizationRequest,
        discover_cancellable,
    },
    skills::{SkillComponentReport, SkillStore},
};

#[derive(Clone)]
pub(crate) struct PluginManager {
    store: PluginStore,
    mcp_catalog: McpCatalogStore,
    mcp_adapter: Option<PathBuf>,
    authorizations: McpAuthorizationResolver,
    catalog: Option<IntegrationCatalog>,
    skills: SkillStore,
}

impl PluginManager {
    pub(crate) fn initialize(
        database: PathBuf,
        packages: PathBuf,
        mcp_catalog: McpCatalogStore,
        mcp_adapter: Option<PathBuf>,
        credentials: McpCredentialResolver,
        catalog: Option<IntegrationCatalog>,
        skills: SkillStore,
    ) -> Result<Self, PluginError> {
        let authorizations =
            McpAuthorizationResolver::new(&mcp_catalog, mcp_adapter.clone(), credentials);
        Ok(Self {
            store: PluginStore::initialize(database, packages)?,
            mcp_catalog,
            mcp_adapter,
            authorizations,
            catalog,
            skills,
        })
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

    pub(crate) async fn search_catalog(
        &self,
        query: &str,
        cancellation: CancellationToken,
    ) -> Result<Vec<CatalogCandidate>, PluginError> {
        let catalog = self.catalog.as_ref().ok_or_else(|| {
            PluginError::Unavailable(
                "RENOA_INTEGRATION_CATALOG_ADAPTER must be set before searching for extensions"
                    .to_owned(),
            )
        })?;
        Ok(catalog.search(query, cancellation).await?)
    }

    pub(crate) async fn add_alpha(
        &self,
        request: ExtensionAddRequest,
        operation_id: &str,
        cancellation: CancellationToken,
    ) -> Result<ExtensionAddOutcome, PluginError> {
        let connection_request = request.connection;
        let prepared = match request.source {
            ExtensionSource::Catalog { reference } => {
                let catalog = self.catalog.as_ref().ok_or_else(|| {
                    PluginError::Unavailable(
                        "RENOA_INTEGRATION_CATALOG_ADAPTER must be set before adding a discovered extension"
                            .to_owned(),
                    )
                })?;
                let candidate = catalog.resolve(&reference, cancellation.clone()).await?;
                let generated = GeneratedMcpPlugin::from_catalog(&candidate);
                let server = generated.server().to_owned();
                let store = self.store.clone();
                let installed =
                    tokio::task::spawn_blocking(move || store.install_generated(&generated))
                        .await??;
                PreparedExtension {
                    installed,
                    source: ExtensionSourceReceipt::Catalog {
                        reference: candidate.reference().to_owned(),
                        name: candidate.name().to_owned(),
                    },
                    generated_server: Some(server),
                    connect_by_default: true,
                }
            }
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
        let skills = self.sync_skills(&prepared.installed).await?;
        self.connect_prepared(
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
        installed: &InstalledPlugin,
    ) -> Result<SkillComponentReport, PluginError> {
        let store = self.store.clone();
        let package_digest = installed.digest().to_owned();
        let plugin_name = installed.metadata().name().to_owned();
        let skills = self.skills.clone();
        tokio::task::spawn_blocking(move || {
            let package_root = store.package_root(&package_digest)?;
            skills
                .sync_plugin(ALPHA_PROFILE_ID, &plugin_name, &package_root)
                .map_err(PluginError::from)
        })
        .await?
    }

    async fn connect_prepared(
        &self,
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
        } = request
            .unwrap_or_else(|| ExtensionConnectionRequest::new(None, None, PluginCredential::None));
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
            .connect_alpha_operation(
                prepared.installed.digest(),
                &server,
                &connection,
                credential,
                operation_id,
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

    pub(crate) async fn connect_alpha(
        &self,
        package_digest: &str,
        server_id: &str,
        connection_id: &str,
        credential: PluginCredential,
        cancellation: CancellationToken,
    ) -> Result<McpCatalogSnapshot, PluginError> {
        let operation_id = format!("host-connect.{}", uuid::Uuid::new_v4());
        self.connect_alpha_operation(
            package_digest,
            server_id,
            connection_id,
            credential,
            &operation_id,
            cancellation,
        )
        .await
    }

    pub(crate) async fn connect_alpha_operation(
        &self,
        package_digest: &str,
        server_id: &str,
        connection_id: &str,
        credential: PluginCredential,
        operation_id: &str,
        cancellation: CancellationToken,
    ) -> Result<McpCatalogSnapshot, PluginError> {
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
                    "package '{}' has no supported MCP server '{server_id}'",
                    plugin.digest()
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
            catalog.register_connection(
                &registered_integration,
                &registered_connection,
                &registered_endpoint,
                &registered_headers_copy,
                &registered_auth,
            )
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
        let stored_snapshot = snapshot.clone();
        tokio::task::spawn_blocking(move || {
            catalog.publish_and_enable_connection(ALPHA_PROFILE_ID, &stored_snapshot)
        })
        .await??;
        Ok(snapshot)
    }

    pub(crate) async fn authorize_alpha(
        &self,
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
        let stored_snapshot = snapshot.clone();
        tokio::task::spawn_blocking(move || {
            catalog.publish_and_enable_connection(ALPHA_PROFILE_ID, &stored_snapshot)
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
    Catalog { reference: String, name: String },
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

fn credential_auth(
    credential: PluginCredential,
    connection_id: &str,
    endpoint: &str,
) -> Result<McpConnectionAuth, PluginError> {
    match credential {
        PluginCredential::None => Ok(McpConnectionAuth::None),
        PluginCredential::SecretServiceBearer { credential_id } => {
            Ok(McpConnectionAuth::secret_service_bearer(&credential_id)?)
        }
        PluginCredential::OAuth => Ok(McpConnectionAuth::oauth(connection_id, endpoint)?),
    }
}

fn integration_id(plugin_digest: &str, server_id: &str) -> String {
    let server_digest = hex(&Sha256::digest(server_id.as_bytes()));
    format!("plugin.{}.{}", &plugin_digest[..24], &server_digest[..24])
}

fn default_connection_id(plugin_digest: &str, server_id: &str) -> String {
    format!("{}.default", integration_id(plugin_digest, server_id))
}

fn hex(bytes: &[u8]) -> String {
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    let mut output = String::with_capacity(bytes.len() * 2);
    for &byte in bytes {
        output.push(char::from(DIGITS[usize::from(byte >> 4)]));
        output.push(char::from(DIGITS[usize::from(byte & 0x0f)]));
    }
    output
}
