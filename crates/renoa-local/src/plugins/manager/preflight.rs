use super::{default_connection_id, integration_id};
use crate::{
    mcp::{
        McpCatalogStore, McpConnectionAuth, McpConnectionCandidate, McpHostError,
        McpOAuthRegistration, McpRequestHeaders, McpStdioConfig,
    },
    plugins::{
        PluginConnectionRequest, PluginCredential, PluginError, PluginMcpServer,
        PluginOAuthRegistration, store::PluginStore,
    },
};

pub(super) fn resolve_server_stdio(
    store: &PluginStore,
    digest: &str,
    server: &PluginMcpServer,
) -> Result<Option<McpStdioConfig>, PluginError> {
    let Some(stdio) = server.stdio() else {
        return Ok(None);
    };
    let root = store.package_path(digest)?;
    let data = store.plugin_data_dir(digest)?;
    Ok(Some(stdio.resolve_for_package(&root, &data)?))
}

pub(super) fn add_connection(
    catalog: &McpCatalogStore,
    store: &PluginStore,
    digest: &str,
    servers: &[PluginMcpServer],
    request: Option<&PluginConnectionRequest>,
    connect_default: bool,
    default_server: Option<&str>,
) -> Result<(), PluginError> {
    if request.is_none() && !connect_default {
        return Ok(());
    }
    let selected = request
        .and_then(|request| request.server.as_deref())
        .or(default_server);
    let server =
        match selected {
            Some(id) => servers
                .iter()
                .find(|server| server.id() == id)
                .ok_or_else(|| {
                    PluginError::Invalid(format!("plugin has no supported MCP server '{id}'"))
                })?,
            None if servers.len() == 1 => &servers[0],
            None => return Err(PluginError::Invalid(
                "adding this package with a connection requires an exact supported MCP server id"
                    .to_owned(),
            )),
        };
    let connection = request
        .and_then(|request| request.id.clone())
        .unwrap_or_else(|| default_connection_id(digest, server.id()));
    crate::mcp::validate_identity("connection", &connection)?;
    let stdio = resolve_server_stdio(store, digest, server)?;
    if stdio.is_some()
        && request.is_some_and(|request| !matches!(request.credential, PluginCredential::None))
    {
        return Err(PluginError::Invalid(
            "stdio MCP servers do not accept HTTP credentials or OAuth; omit credential".to_owned(),
        ));
    }
    let endpoint = match &stdio {
        Some(resolved) => resolved.canonical_endpoint()?,
        None => server.endpoint().to_owned(),
    };
    let auth = preflight_auth(
        catalog,
        &connection,
        &endpoint,
        request.map(|request| &request.credential),
        request.is_some_and(|request| request.replace),
    )?;
    let candidate = McpConnectionCandidate::new(
        integration_id(digest, server.id()),
        connection,
        endpoint,
        McpRequestHeaders::new(
            server
                .request_headers()
                .iter()
                .map(|(name, value)| (name.clone(), value.clone())),
        )?,
        stdio,
        auth,
    )?;
    catalog.preflight_connection(&candidate, request.is_some_and(|request| request.replace))?;
    Ok(())
}

fn preflight_auth(
    catalog: &McpCatalogStore,
    connection: &str,
    endpoint: &str,
    credential: Option<&PluginCredential>,
    replace: bool,
) -> Result<McpConnectionAuth, PluginError> {
    Ok(match credential {
        None | Some(PluginCredential::None) => McpConnectionAuth::None,
        Some(PluginCredential::SecretServiceBearer { credential_id }) => {
            McpConnectionAuth::secret_service_bearer(credential_id)?
        }
        Some(PluginCredential::SecretServiceHeader {
            credential_id,
            header,
            prefix,
        }) => McpConnectionAuth::secret_service_header(credential_id, header, prefix)?,
        Some(PluginCredential::OAuth { registration }) => {
            let registration = match registration {
                PluginOAuthRegistration::Auto => {
                    // Automatic registration is discovered later. Existing OAuth
                    // configuration can still prove all statically known conflicts.
                    return match catalog.connection_config(connection) {
                        Ok(config)
                            if matches!(config.auth, McpConnectionAuth::OAuth { .. })
                                && config.endpoint == endpoint =>
                        {
                            Ok(config.auth)
                        }
                        Ok(_) if !replace => Err(McpHostError::Conflict(format!(
                            "connection '{connection}' already uses a different endpoint or authentication kind"
                        ))
                        .into()),
                        Ok(_) | Err(McpHostError::NotFound(_)) => Ok(McpConnectionAuth::None),
                        Err(error) => Err(error.into()),
                    };
                }
                PluginOAuthRegistration::Dynamic => McpOAuthRegistration::dynamic(),
                PluginOAuthRegistration::ClientMetadata { url } => {
                    McpOAuthRegistration::client_metadata(url)?
                }
                PluginOAuthRegistration::PreRegistered { credential_id } => {
                    McpOAuthRegistration::pre_registered(credential_id)?
                }
            };
            McpConnectionAuth::oauth(connection, endpoint, registration)?
        }
    })
}
