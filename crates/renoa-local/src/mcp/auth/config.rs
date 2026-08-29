use serde::{Deserialize, Serialize};

use super::{validate_account, validate_hostname};
use crate::mcp::{McpHostError, hex_sha256, validate_endpoint, validate_identity};

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub(crate) enum McpConnectionAuth {
    None,
    GhCli {
        hostname: String,
        account: String,
    },
    SecretServiceBearer {
        credential_id: String,
    },
    OAuth {
        credential_id: String,
        registration: McpOAuthRegistration,
    },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "mode", rename_all = "snake_case", deny_unknown_fields)]
pub(crate) enum McpOAuthRegistration {
    Dynamic,
    ClientMetadata { url: String },
    PreRegistered { credential_id: String },
}

impl McpOAuthRegistration {
    pub(crate) const fn dynamic() -> Self {
        Self::Dynamic
    }

    pub(crate) fn client_metadata(url: &str) -> Result<Self, McpHostError> {
        let parsed = url::Url::parse(url).map_err(|error| {
            McpHostError::Invalid(format!("OAuth client metadata URL is invalid: {error}"))
        })?;
        if parsed.scheme() != "https"
            || parsed.host_str().is_none()
            || parsed.path() == "/"
            || !parsed.username().is_empty()
            || parsed.password().is_some()
            || parsed.fragment().is_some()
        {
            return Err(McpHostError::Invalid(
                "OAuth client metadata URL must use HTTPS, have a non-root path, and contain no credentials or fragment"
                    .to_owned(),
            ));
        }
        Ok(Self::ClientMetadata {
            url: parsed.to_string(),
        })
    }

    pub(crate) fn pre_registered(credential_id: &str) -> Result<Self, McpHostError> {
        validate_identity("OAuth client credential", credential_id)?;
        Ok(Self::PreRegistered {
            credential_id: credential_id.to_owned(),
        })
    }

    fn validate(self) -> Result<Self, McpHostError> {
        match self {
            Self::Dynamic => Ok(Self::Dynamic),
            Self::ClientMetadata { url } => Self::client_metadata(&url),
            Self::PreRegistered { credential_id } => Self::pre_registered(&credential_id),
        }
    }
}

impl McpConnectionAuth {
    pub(crate) fn gh_cli(hostname: &str, account: &str) -> Result<Self, McpHostError> {
        validate_hostname(hostname)?;
        validate_account(account)?;
        Ok(Self::GhCli {
            hostname: hostname.to_ascii_lowercase(),
            account: account.to_owned(),
        })
    }

    pub(crate) fn secret_service_bearer(credential_id: &str) -> Result<Self, McpHostError> {
        validate_identity("credential", credential_id)?;
        Ok(Self::SecretServiceBearer {
            credential_id: credential_id.to_owned(),
        })
    }

    pub(crate) fn oauth(
        connection_id: &str,
        endpoint: &str,
        registration: McpOAuthRegistration,
    ) -> Result<Self, McpHostError> {
        validate_identity("connection", connection_id)?;
        validate_endpoint(endpoint)?;
        let registration = registration.validate()?;
        let binding = match &registration {
            McpOAuthRegistration::Dynamic => format!("{connection_id}\0{endpoint}"),
            _ => format!(
                "{connection_id}\0{endpoint}\0{}",
                serde_json::to_string(&registration)?
            ),
        };
        let digest = hex_sha256(binding.as_bytes());
        Ok(Self::OAuth {
            credential_id: format!("oauth.{digest}"),
            registration,
        })
    }

    pub(crate) fn from_stored(
        kind: &str,
        hostname: Option<String>,
        account: Option<String>,
        credential_id: Option<String>,
        oauth_registration: Option<String>,
    ) -> Result<Self, McpHostError> {
        match (kind, hostname, account, credential_id, oauth_registration) {
            ("none", None, None, None, None) => Ok(Self::None),
            ("gh_cli", Some(hostname), Some(account), None, None) => {
                Self::gh_cli(&hostname, &account)
            }
            ("secret_service_bearer", None, None, Some(credential_id), None) => {
                Self::secret_service_bearer(&credential_id)
            }
            ("oauth", None, None, Some(credential_id), Some(registration)) => {
                validate_identity("credential", &credential_id)?;
                let registration = serde_json::from_str::<McpOAuthRegistration>(&registration)
                    .map_err(|_| {
                        McpHostError::Invalid(
                            "stored MCP OAuth registration is malformed".to_owned(),
                        )
                    })?
                    .validate()?;
                Ok(Self::OAuth {
                    credential_id,
                    registration,
                })
            }
            _ => Err(McpHostError::Invalid(
                "stored MCP credential reference is malformed".to_owned(),
            )),
        }
    }

    pub(crate) const fn stored_kind(&self) -> &'static str {
        match self {
            Self::None => "none",
            Self::GhCli { .. } => "gh_cli",
            Self::SecretServiceBearer { .. } => "secret_service_bearer",
            Self::OAuth { .. } => "oauth",
        }
    }

    pub(crate) fn stored_hostname(&self) -> Option<&str> {
        match self {
            Self::GhCli { hostname, .. } => Some(hostname),
            Self::None | Self::SecretServiceBearer { .. } | Self::OAuth { .. } => None,
        }
    }

    pub(crate) fn stored_account(&self) -> Option<&str> {
        match self {
            Self::GhCli { account, .. } => Some(account),
            Self::None | Self::SecretServiceBearer { .. } | Self::OAuth { .. } => None,
        }
    }

    pub(crate) fn stored_credential_id(&self) -> Option<&str> {
        match self {
            Self::SecretServiceBearer { credential_id } | Self::OAuth { credential_id, .. } => {
                Some(credential_id)
            }
            Self::None | Self::GhCli { .. } => None,
        }
    }

    pub(crate) fn stored_oauth_registration(&self) -> Result<Option<String>, McpHostError> {
        match self {
            Self::OAuth { registration, .. } => Ok(Some(serde_json::to_string(registration)?)),
            Self::None | Self::GhCli { .. } | Self::SecretServiceBearer { .. } => Ok(None),
        }
    }

    pub(crate) fn oauth_credential_id(&self) -> Option<&str> {
        match self {
            Self::OAuth { credential_id, .. } => Some(credential_id),
            Self::None | Self::GhCli { .. } | Self::SecretServiceBearer { .. } => None,
        }
    }

    pub(crate) fn oauth_registration(&self) -> Option<&McpOAuthRegistration> {
        match self {
            Self::OAuth { registration, .. } => Some(registration),
            Self::None | Self::GhCli { .. } | Self::SecretServiceBearer { .. } => None,
        }
    }

    pub(crate) fn validate_oauth_binding(
        &self,
        connection_id: &str,
        endpoint: &str,
    ) -> Result<(), McpHostError> {
        if let Self::OAuth { registration, .. } = self
            && *self != Self::oauth(connection_id, endpoint, registration.clone())?
        {
            return Err(McpHostError::Invalid(
                "stored MCP OAuth credential reference does not match its connection".to_owned(),
            ));
        }
        Ok(())
    }
}
