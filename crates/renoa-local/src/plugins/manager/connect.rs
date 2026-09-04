use renoa_agent::ToolUpdates;
use tokio_util::sync::CancellationToken;

use super::{PluginManager, credential::credential_auth, identity::integration_id};
use crate::{
    AgentProfileId,
    mcp::{
        McpAdapterError, McpCatalogSnapshot, McpConnectionCandidate, McpHostError,
        McpOAuthAuthorizationRequest, McpOAuthError, McpRequestHeaders, discover_cancellable,
    },
    plugins::{PluginCredential, PluginError},
};

pub(crate) struct ProfileConnectionRequest<'a> {
    pub(crate) profile_id: &'a AgentProfileId,
    pub(crate) package_digest: &'a str,
    pub(crate) server_id: &'a str,
    pub(crate) connection_id: &'a str,
    pub(crate) credential: PluginCredential,
    pub(crate) replace: bool,
    pub(crate) requested_scope: Option<&'a str>,
    pub(crate) operation_id: &'a str,
    pub(crate) updates: Option<&'a ToolUpdates>,
}

pub(crate) struct ProfileAuthorizationRequest<'a> {
    pub(crate) profile_id: &'a AgentProfileId,
    pub(crate) connection_id: &'a str,
    pub(crate) operation_id: &'a str,
    pub(crate) restart: bool,
    pub(crate) requested_scope: Option<&'a str>,
    pub(crate) updates: Option<&'a ToolUpdates>,
}

impl PluginManager {
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
                requested_scope: None,
                operation_id: &operation_id,
                updates: None,
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
        let PreparedConnection {
            adapter,
            candidate,
            display_name,
        } = self
            .prepare_connection(
                request.package_digest,
                request.server_id,
                request.connection_id,
                request.credential,
                request.replace,
                cancellation.clone(),
            )
            .await?;
        self.authorizations
            .ensure_credentials(
                candidate.auth(),
                request.operation_id,
                request.updates,
                cancellation.clone(),
            )
            .await?;

        let oauth_request = || McpOAuthAuthorizationRequest {
            connection_id: candidate.connection_id(),
            display_name: Some(display_name.as_str()),
            endpoint: candidate.endpoint(),
            reference: candidate.auth(),
            operation_id: request.operation_id,
            restart: false,
            requested_scope: request.requested_scope,
            updates: request.updates,
        };
        let authorization = if request.requested_scope.is_some() {
            Some(
                self.authorizations
                    .authorize(oauth_request(), cancellation.clone())
                    .await?,
            )
        } else {
            match self
                .authorizations
                .resolve(
                    candidate.connection_id(),
                    candidate.endpoint(),
                    candidate.auth(),
                    request.operation_id,
                    cancellation.clone(),
                )
                .await
            {
                Ok(authorization) => authorization,
                Err(McpHostError::OAuth(McpOAuthError::AuthorizationRequired(_)))
                    if request.updates.is_some() =>
                {
                    Some(
                        self.authorizations
                            .authorize(oauth_request(), cancellation.clone())
                            .await?,
                    )
                }
                Err(error) => return Err(error.into()),
            }
        };
        if cancellation.is_cancelled() {
            return Err(McpHostError::from(McpAdapterError::Cancelled).into());
        }
        let snapshot = discover_cancellable(
            &adapter,
            candidate.connection_id(),
            candidate.endpoint(),
            candidate.request_headers(),
            authorization.as_ref(),
            cancellation,
        )
        .await?;
        let catalog = self.mcp_catalog.clone();
        let profile_id = request.profile_id.clone();
        let committed = candidate.clone();
        let stored_snapshot = snapshot.clone();
        tokio::task::spawn_blocking(move || {
            catalog.commit_connection(
                profile_id.as_str(),
                &committed,
                &stored_snapshot,
                request.replace,
            )
        })
        .await??;
        Ok(snapshot)
    }

    async fn prepare_connection(
        &self,
        package_digest: &str,
        server_id: &str,
        connection_id: &str,
        credential: PluginCredential,
        replace: bool,
        cancellation: CancellationToken,
    ) -> Result<PreparedConnection, PluginError> {
        let plugin = self.load_available(package_digest).await?;
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
        let auth = credential_auth(
            credential,
            connection_id,
            server.endpoint(),
            &self.authorizations,
            cancellation,
        )
        .await?;
        let headers = McpRequestHeaders::new(
            server
                .request_headers()
                .iter()
                .map(|(name, value)| (name.clone(), value.clone())),
        )?;
        let candidate = McpConnectionCandidate::new(
            integration_id(plugin.digest(), server.id()),
            connection_id.to_owned(),
            server.endpoint().to_owned(),
            headers,
            auth,
        )?;
        let catalog = self.mcp_catalog.clone();
        let preflight = candidate.clone();
        tokio::task::spawn_blocking(move || catalog.preflight_connection(&preflight, replace))
            .await??;
        Ok(PreparedConnection {
            adapter,
            candidate,
            display_name: plugin.metadata().name().to_owned(),
        })
    }

    pub(crate) async fn authorize_profile(
        &self,
        request: ProfileAuthorizationRequest<'_>,
        cancellation: CancellationToken,
    ) -> Result<McpCatalogSnapshot, PluginError> {
        let catalog = self.mcp_catalog.clone();
        let stored_connection = request.connection_id.to_owned();
        let connection =
            tokio::task::spawn_blocking(move || catalog.connection_config(&stored_connection))
                .await??;
        self.authorizations
            .ensure_credentials(
                &connection.auth,
                request.operation_id,
                request.updates,
                cancellation.clone(),
            )
            .await?;
        let authorization = self
            .authorizations
            .authorize(
                McpOAuthAuthorizationRequest {
                    connection_id: request.connection_id,
                    display_name: None,
                    endpoint: &connection.endpoint,
                    reference: &connection.auth,
                    operation_id: request.operation_id,
                    restart: request.restart,
                    requested_scope: request.requested_scope,
                    updates: request.updates,
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
            request.connection_id,
            &connection.endpoint,
            &connection.request_headers,
            Some(&authorization),
            cancellation,
        )
        .await?;
        let catalog = self.mcp_catalog.clone();
        let profile_id = request.profile_id.clone();
        let stored_snapshot = snapshot.clone();
        tokio::task::spawn_blocking(move || {
            catalog.publish_and_enable_connection(profile_id.as_str(), &stored_snapshot)
        })
        .await??;
        Ok(snapshot)
    }
}

struct PreparedConnection {
    adapter: std::path::PathBuf,
    candidate: McpConnectionCandidate,
    display_name: String,
}
