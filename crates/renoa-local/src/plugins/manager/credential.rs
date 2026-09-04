use super::super::{PluginCredential, PluginError, PluginOAuthRegistration};
use crate::mcp::{McpAuthorizationResolver, McpConnectionAuth, McpOAuthRegistration};
use tokio_util::sync::CancellationToken;

pub(super) async fn credential_auth(
    credential: PluginCredential,
    connection_id: &str,
    endpoint: &str,
    authorizations: &McpAuthorizationResolver,
    cancellation: CancellationToken,
) -> Result<McpConnectionAuth, PluginError> {
    match credential {
        PluginCredential::None => Ok(McpConnectionAuth::None),
        PluginCredential::SecretServiceBearer { credential_id } => {
            Ok(McpConnectionAuth::secret_service_bearer(&credential_id)?)
        }
        PluginCredential::SecretServiceHeader {
            credential_id,
            header,
            prefix,
        } => Ok(McpConnectionAuth::secret_service_header(
            &credential_id,
            &header,
            &prefix,
        )?),
        PluginCredential::OAuth { registration } => {
            let registration = match registration {
                PluginOAuthRegistration::Auto => {
                    return Ok(authorizations
                        .automatic_oauth(connection_id, endpoint, cancellation)
                        .await?);
                }
                PluginOAuthRegistration::Dynamic => McpOAuthRegistration::dynamic(),
                PluginOAuthRegistration::ClientMetadata { url } => {
                    McpOAuthRegistration::client_metadata(&url)?
                }
                PluginOAuthRegistration::PreRegistered { credential_id } => {
                    McpOAuthRegistration::pre_registered(&credential_id)?
                }
            };
            Ok(McpConnectionAuth::oauth(
                connection_id,
                endpoint,
                registration,
            )?)
        }
    }
}
