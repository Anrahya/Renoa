mod browser;
mod callback;
mod completion;
mod interactive;
mod lock;
mod process;
mod resolution;
mod secret;
mod sensitive;
mod store;

#[cfg(test)]
mod tests;

use std::{path::PathBuf, time::Duration};

use renoa_agent::ToolUpdates;
use renoa_kernel::{CommandId, SessionId};
use tokio_util::sync::CancellationToken;

use super::{
    McpAdapterError, McpCatalogStore, McpConnectionAuth, McpCredentialHeader,
    McpCredentialResolver, McpHostError, McpOAuthError, McpOAuthRegistration,
};
use secret::OAuthSecretStore;
use sensitive::SensitiveString;
use store::OAuthFlowStore;

const INTERACTIVE_LOCK_WAIT: Duration = Duration::from_secs(2);
const REFRESH_LOCK_WAIT: Duration = Duration::from_secs(30);

#[derive(Clone)]
pub(crate) struct McpAuthorizationResolver {
    credentials: McpCredentialResolver,
    oauth: OAuthCoordinator,
}

pub(crate) struct McpOAuthAuthorizationRequest<'a> {
    pub(crate) connection_id: &'a str,
    pub(crate) endpoint: &'a str,
    pub(crate) reference: &'a McpConnectionAuth,
    pub(crate) operation_id: &'a str,
    pub(crate) restart: bool,
    pub(crate) updates: Option<&'a ToolUpdates>,
}

#[derive(Clone)]
struct OAuthCoordinator {
    adapter: Option<PathBuf>,
    browser: PathBuf,
    locks: PathBuf,
    secrets: OAuthSecretStore,
    flows: OAuthFlowStore,
}

impl McpAuthorizationResolver {
    pub(crate) fn new(
        catalog: &McpCatalogStore,
        adapter: Option<PathBuf>,
        credentials: McpCredentialResolver,
    ) -> Self {
        let data_root = catalog
            .path()
            .parent()
            .unwrap_or_else(|| std::path::Path::new("."));
        let oauth = OAuthCoordinator {
            adapter,
            browser: PathBuf::from("xdg-open"),
            locks: data_root.join("oauth-locks"),
            secrets: OAuthSecretStore::new(credentials.secret_tool_executable()),
            flows: OAuthFlowStore::new(catalog.path().to_path_buf()),
        };
        Self { credentials, oauth }
    }

    pub(crate) async fn resolve(
        &self,
        connection_id: &str,
        endpoint: &str,
        reference: &McpConnectionAuth,
        operation_id: &str,
        cancellation: CancellationToken,
    ) -> Result<Option<McpCredentialHeader>, McpHostError> {
        if reference.oauth_credential_id().is_some() {
            return self
                .oauth
                .resolve(
                    connection_id,
                    endpoint,
                    reference,
                    operation_id,
                    cancellation,
                )
                .await
                .map(Some);
        }
        self.credentials
            .resolve(reference, cancellation)
            .await
            .map_err(McpAdapterError::from)
            .map_err(McpHostError::from)
    }

    pub(crate) async fn authorize(
        &self,
        request: McpOAuthAuthorizationRequest<'_>,
        cancellation: CancellationToken,
    ) -> Result<McpCredentialHeader, McpHostError> {
        if request.reference.oauth_credential_id().is_none() {
            return Err(McpOAuthError::Invalid(format!(
                "connection '{}' is not configured for OAuth",
                request.connection_id
            ))
            .into());
        }
        self.oauth.authorize(request, cancellation).await
    }
}

impl OAuthCoordinator {
    fn adapter(&self) -> Result<&std::path::Path, McpHostError> {
        self.adapter.as_deref().ok_or_else(|| {
            McpHostError::Adapter(McpAdapterError::Protocol(
                "RENOA_MCP_ADAPTER must be set before MCP OAuth can run".to_owned(),
            ))
        })
    }

    fn credential_id(reference: &McpConnectionAuth) -> Result<&str, McpHostError> {
        reference.oauth_credential_id().ok_or_else(|| {
            McpOAuthError::Invalid("connection credential kind is not OAuth".to_owned()).into()
        })
    }

    async fn adapter_registration(
        &self,
        reference: &McpConnectionAuth,
        cancellation: CancellationToken,
    ) -> Result<process::OAuthRegistration, McpHostError> {
        match reference.oauth_registration().ok_or_else(|| {
            McpOAuthError::Invalid("connection credential kind is not OAuth".to_owned())
        })? {
            McpOAuthRegistration::Dynamic => Ok(process::OAuthRegistration::Dynamic),
            McpOAuthRegistration::ClientMetadata { url } => {
                Ok(process::OAuthRegistration::ClientMetadata {
                    client_metadata_url: url.clone(),
                })
            }
            McpOAuthRegistration::PreRegistered { credential_id } => {
                let client = self
                    .secrets
                    .load_pre_registered_client(credential_id, cancellation)
                    .await?;
                Ok(process::OAuthRegistration::PreRegistered {
                    issuer: client.issuer,
                    client_id: client.client_id,
                    client_secret: client.client_secret,
                })
            }
        }
    }
}

fn adapter_may_have_dispatched(error: &McpAdapterError) -> bool {
    !matches!(
        error,
        McpAdapterError::Resolve(_)
            | McpAdapterError::NotFile(_)
            | McpAdapterError::Encode(_)
            | McpAdapterError::InputLimit
            | McpAdapterError::Start(_)
    )
}

pub(crate) fn operation_id(
    session_id: SessionId,
    command_id: Option<CommandId>,
    call_id: &str,
) -> String {
    match command_id {
        Some(command_id) => format!("{session_id}/{command_id}/{call_id}"),
        None => format!("{session_id}/configuration/{call_id}"),
    }
}
