use std::{
    fs,
    path::{Path, PathBuf},
};

use renoa_agent::ToolUpdates;
use tokio_util::sync::CancellationToken;

use super::{PluginManager, credential::credential_auth, identity::integration_id};
use renoa_kernel::AgentId;

use crate::{
    mcp::{
        McpAdapterError, McpCatalogSnapshot, McpConnectionCandidate, McpCredentialHeader,
        McpHostError, McpOAuthAuthorizationRequest, McpOAuthError, McpRequestHeaders,
        McpStdioConfig, discover_cancellable,
    },
    plugins::{PluginCredential, PluginError},
};

pub(crate) struct ProfileConnectionRequest<'a> {
    pub(crate) agent_id: &'a AgentId,
    pub(crate) package_digest: &'a str,
    pub(crate) server_id: &'a str,
    pub(crate) connection_id: &'a str,
    pub(crate) credential: PluginCredential,
    pub(crate) replace: bool,
    pub(crate) restart: bool,
    pub(crate) requested_scope: Option<&'a str>,
    pub(crate) operation_id: &'a str,
    pub(crate) updates: Option<&'a ToolUpdates>,
}

pub(crate) struct ProfileAuthorizationRequest<'a> {
    pub(crate) agent_id: &'a AgentId,
    pub(crate) connection_id: &'a str,
    pub(crate) operation_id: &'a str,
    pub(crate) restart: bool,
    pub(crate) requested_scope: Option<&'a str>,
    pub(crate) updates: Option<&'a ToolUpdates>,
}

impl PluginManager {
    pub(crate) async fn connect_profile(
        &self,
        agent_id: &AgentId,
        package_digest: &str,
        server_id: &str,
        connection_id: &str,
        credential: PluginCredential,
        cancellation: CancellationToken,
    ) -> Result<McpCatalogSnapshot, PluginError> {
        let operation_id = format!("host-connect.{}", uuid::Uuid::new_v4());
        self.connect_profile_operation(
            ProfileConnectionRequest {
                agent_id,
                package_digest,
                server_id,
                connection_id,
                credential,
                replace: false,
                restart: false,
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
        self.preflight_revision(request.agent_id, request.package_digest)
            .await?;
        let PreparedConnection {
            adapter,
            candidate,
            display_name,
            plugin_data_dir,
        } = self
            .prepare_connection(&request, cancellation.clone())
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
            restart: request.restart,
            requested_scope: request.requested_scope,
            updates: request.updates,
        };
        let authorization = if request.restart || request.requested_scope.is_some() {
            Some(self.sign_in(oauth_request(), cancellation.clone()).await?)
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
                Err(McpHostError::OAuth(McpOAuthError::AuthorizationRequired(_))) => {
                    Some(self.sign_in(oauth_request(), cancellation.clone()).await?)
                }
                Err(error) => return Err(error.into()),
            }
        };
        if cancellation.is_cancelled() {
            return Err(McpHostError::from(McpAdapterError::Cancelled).into());
        }
        let created_data = match plugin_data_dir.clone() {
            Some(dir) => {
                tokio::task::spawn_blocking(move || ensure_plugin_data_dir(&dir)).await??
            }
            None => false,
        };
        let connected = self
            .discover_and_commit(&request, &adapter, &candidate, authorization, cancellation)
            .await;
        if connected.is_err()
            && created_data
            && let Some(dir) = plugin_data_dir
        {
            // A failed connect removes the data directory it made while it is
            // still empty, so one another connect has begun to use is kept.
            // Should that fail, the connect error is the one to report: a
            // retry adopts the directory left behind.
            let _removal = tokio::task::spawn_blocking(move || fs::remove_dir(dir)).await;
        }
        connected
    }

    async fn discover_and_commit(
        &self,
        request: &ProfileConnectionRequest<'_>,
        adapter: &Path,
        candidate: &McpConnectionCandidate,
        authorization: Option<McpCredentialHeader>,
        cancellation: CancellationToken,
    ) -> Result<McpCatalogSnapshot, PluginError> {
        let snapshot = discover_cancellable(
            adapter,
            candidate.connection_id(),
            candidate.endpoint(),
            candidate.request_headers(),
            candidate.stdio(),
            authorization.as_ref(),
            cancellation,
        )
        .await?;
        let catalog = self.mcp_catalog.clone();
        let agent_id = *request.agent_id;
        let committed = candidate.clone();
        let stored_snapshot = snapshot.clone();
        let replace = request.replace;
        tokio::task::spawn_blocking(move || {
            catalog.commit_connection(&agent_id.to_string(), &committed, &stored_snapshot, replace)
        })
        .await??;
        Ok(snapshot)
    }

    async fn prepare_connection(
        &self,
        request: &ProfileConnectionRequest<'_>,
        cancellation: CancellationToken,
    ) -> Result<PreparedConnection, PluginError> {
        let plugin = self.load_available(request.package_digest).await?;
        let server = plugin
            .mcp_servers()
            .iter()
            .find(|server| server.id() == request.server_id)
            .cloned()
            .ok_or_else(|| {
                PluginError::NotFound(format!(
                    "package '{}' has no supported MCP server '{}'",
                    plugin.digest(),
                    request.server_id,
                ))
            })?;
        if server.stdio().is_some() && (request.restart || request.requested_scope.is_some()) {
            return Err(PluginError::Invalid(
                "stdio MCP connections do not accept OAuth restart or scope options".to_owned(),
            ));
        }
        let adapter = self.mcp_adapter.clone().ok_or_else(|| {
            PluginError::Unavailable(
                "RENOA_MCP_ADAPTER must be set before connecting a package MCP server".to_owned(),
            )
        })?;
        let preflight_catalog = self.mcp_catalog.clone();
        let preflight_store = self.store.clone();
        let preflight_digest = request.package_digest.to_owned();
        let agent = *request.agent_id;
        let preflight_servers = plugin.mcp_servers().to_vec();
        let preflight_request = crate::plugins::PluginConnectionRequest::new(
            Some(request.connection_id.to_owned()),
            Some(request.server_id.to_owned()),
            request.credential.clone(),
            request.replace,
        );
        tokio::task::spawn_blocking(move || {
            super::preflight::add_connection(
                &preflight_catalog,
                &preflight_store,
                agent,
                &preflight_digest,
                &preflight_servers,
                Some(&preflight_request),
                false,
                None,
            )
        })
        .await??;
        let stdio = super::preflight::resolve_server_stdio(&self.store, plugin.digest(), &server)?;
        let endpoint = match &stdio {
            Some(resolved) => resolved.canonical_endpoint()?,
            None => server.endpoint().to_owned(),
        };
        let plugin_data_dir = server
            .stdio()
            .is_some_and(McpStdioConfig::exposes_plugin_dirs)
            .then(|| self.store.plugin_data_dir(plugin.digest()))
            .transpose()?;
        let auth = credential_auth(
            request.credential.clone(),
            request.connection_id,
            &endpoint,
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
            request.connection_id.to_owned(),
            endpoint,
            headers,
            stdio,
            auth,
        )?;
        let catalog = self.mcp_catalog.clone();
        let preflight = candidate.clone();
        let replace = request.replace;
        tokio::task::spawn_blocking(move || catalog.preflight_connection(&preflight, replace))
            .await??;
        Ok(PreparedConnection {
            adapter,
            candidate,
            display_name: plugin.metadata().name().to_owned(),
            plugin_data_dir,
        })
    }

    /// Signs in when someone can be shown the link, or when this operation
    /// already showed one the person may have answered. Any other sign-in
    /// would wait on a link nobody sees, so it is refused as needing them.
    async fn sign_in(
        &self,
        request: McpOAuthAuthorizationRequest<'_>,
        cancellation: CancellationToken,
    ) -> Result<McpCredentialHeader, McpHostError> {
        if request.updates.is_none()
            && !self
                .authorizations
                .shown_by(request.connection_id, request.operation_id)
                .await?
        {
            return Err(
                McpOAuthError::AuthorizationRequired(request.connection_id.to_owned()).into(),
            );
        }
        self.authorizations.authorize(request, cancellation).await
    }

    pub(crate) async fn authorize_profile(
        &self,
        request: ProfileAuthorizationRequest<'_>,
        cancellation: CancellationToken,
    ) -> Result<McpCatalogSnapshot, PluginError> {
        self.preflight_account(request.agent_id, request.connection_id)
            .await?;
        let catalog = self.mcp_catalog.clone();
        let stored_connection = request.connection_id.to_owned();
        let connection =
            tokio::task::spawn_blocking(move || catalog.connection_config(&stored_connection))
                .await??;
        if connection.stdio.is_some() {
            return Err(PluginError::Invalid(
                "stdio MCP connections do not use OAuth authorization".to_owned(),
            ));
        }
        self.authorizations
            .ensure_credentials(
                &connection.auth,
                request.operation_id,
                request.updates,
                cancellation.clone(),
            )
            .await?;
        let authorization = self
            .sign_in(
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
            connection.stdio.as_ref(),
            Some(&authorization),
            cancellation,
        )
        .await?;
        let catalog = self.mcp_catalog.clone();
        let agent_id = *request.agent_id;
        let stored_snapshot = snapshot.clone();
        tokio::task::spawn_blocking(move || {
            catalog.publish_and_enable_connection(&agent_id.to_string(), &stored_snapshot)
        })
        .await??;
        Ok(snapshot)
    }
}

struct PreparedConnection {
    adapter: PathBuf,
    candidate: McpConnectionCandidate,
    display_name: String,
    plugin_data_dir: Option<PathBuf>,
}

/// Creates the package's data directory, private to this user from the
/// start, and says whether this call made it.
fn ensure_plugin_data_dir(path: &Path) -> Result<bool, PluginError> {
    let io = |action| {
        move |source| PluginError::Io {
            action,
            path: path.to_path_buf(),
            source,
        }
    };
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(io("create plugin data root"))?;
    }
    let mut builder = fs::DirBuilder::new();
    #[cfg(unix)]
    std::os::unix::fs::DirBuilderExt::mode(&mut builder, 0o700);
    match builder.create(path) {
        Ok(()) => Ok(true),
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists && path.is_dir() => {
            Ok(false)
        }
        Err(error) => Err(io("create plugin data directory")(error)),
    }
}
