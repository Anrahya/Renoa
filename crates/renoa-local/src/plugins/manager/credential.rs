use super::super::{PluginCredential, PluginError, PluginOAuthRegistration};
use crate::mcp::{McpConnectionAuth, McpOAuthRegistration};

pub(super) fn credential_auth(
    credential: PluginCredential,
    connection_id: &str,
    endpoint: &str,
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
