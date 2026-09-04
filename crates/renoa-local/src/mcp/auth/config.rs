use serde::{Deserialize, Serialize};

use super::{validate_account, validate_hostname};
use crate::mcp::{McpHostError, hex_sha256, validate_endpoint, validate_identity};

const HEADER_TOKEN: &[u8] = b"!#$%&'*+-.^_`|~";
const FORBIDDEN_CREDENTIAL_HEADERS: &[&str] = &[
    "accept",
    "connection",
    "content-length",
    "content-type",
    "host",
    "keep-alive",
    "mcp-method",
    "mcp-protocol-version",
    "mcp-session-id",
    "proxy-authorization",
    "set-cookie",
    "te",
    "trailer",
    "transfer-encoding",
    "upgrade",
];
const MAX_CREDENTIAL_PREFIX_BYTES: usize = 256;

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
    SecretServiceHeader {
        credential_id: String,
        header: String,
        prefix: String,
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
    ClientMetadata {
        url: String,
    },
    PreRegistered {
        credential_id: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        issuer: Option<String>,
    },
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
            issuer: None,
        })
    }

    pub(crate) fn pre_registered_for_issuer(
        credential_id: &str,
        issuer: &str,
    ) -> Result<Self, McpHostError> {
        validate_identity("OAuth client credential", credential_id)?;
        let issuer =
            crate::mcp::oauth::secret_store::validate_issuer(issuer).map_err(|reason| {
                McpHostError::Invalid(format!("OAuth issuer is invalid: {reason}"))
            })?;
        Ok(Self::PreRegistered {
            credential_id: credential_id.to_owned(),
            issuer: Some(issuer),
        })
    }

    fn validate(self) -> Result<Self, McpHostError> {
        match self {
            Self::Dynamic => Ok(Self::Dynamic),
            Self::ClientMetadata { url } => Self::client_metadata(&url),
            Self::PreRegistered {
                credential_id,
                issuer: Some(issuer),
            } => Self::pre_registered_for_issuer(&credential_id, &issuer),
            Self::PreRegistered {
                credential_id,
                issuer: None,
            } => Self::pre_registered(&credential_id),
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

    pub(crate) fn secret_service_header(
        credential_id: &str,
        header: &str,
        prefix: &str,
    ) -> Result<Self, McpHostError> {
        validate_identity("credential", credential_id)?;
        validate_credential_header(header)?;
        validate_credential_prefix(prefix)?;
        Ok(Self::SecretServiceHeader {
            credential_id: credential_id.to_owned(),
            header: header.to_ascii_lowercase(),
            prefix: prefix.to_owned(),
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
        auth_header: Option<String>,
        auth_prefix: Option<String>,
    ) -> Result<Self, McpHostError> {
        match (
            kind,
            hostname,
            account,
            credential_id,
            oauth_registration,
            auth_header,
            auth_prefix,
        ) {
            ("none", None, None, None, None, None, None) => Ok(Self::None),
            ("gh_cli", Some(hostname), Some(account), None, None, None, None) => {
                Self::gh_cli(&hostname, &account)
            }
            ("secret_service_bearer", None, None, Some(credential_id), None, None, None) => {
                Self::secret_service_bearer(&credential_id)
            }
            (
                "secret_service_header",
                None,
                None,
                Some(credential_id),
                None,
                Some(header),
                Some(prefix),
            ) => Self::secret_service_header(&credential_id, &header, &prefix),
            ("oauth", None, None, Some(credential_id), Some(registration), None, None) => {
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
            Self::SecretServiceHeader { .. } => "secret_service_header",
            Self::OAuth { .. } => "oauth",
        }
    }

    pub(crate) fn stored_hostname(&self) -> Option<&str> {
        match self {
            Self::GhCli { hostname, .. } => Some(hostname),
            Self::None
            | Self::SecretServiceBearer { .. }
            | Self::SecretServiceHeader { .. }
            | Self::OAuth { .. } => None,
        }
    }

    pub(crate) fn stored_account(&self) -> Option<&str> {
        match self {
            Self::GhCli { account, .. } => Some(account),
            Self::None
            | Self::SecretServiceBearer { .. }
            | Self::SecretServiceHeader { .. }
            | Self::OAuth { .. } => None,
        }
    }

    pub(crate) fn stored_credential_id(&self) -> Option<&str> {
        match self {
            Self::SecretServiceBearer { credential_id }
            | Self::SecretServiceHeader { credential_id, .. }
            | Self::OAuth { credential_id, .. } => Some(credential_id),
            Self::None | Self::GhCli { .. } => None,
        }
    }

    pub(crate) fn stored_header(&self) -> Option<&str> {
        match self {
            Self::SecretServiceHeader { header, .. } => Some(header),
            Self::None
            | Self::GhCli { .. }
            | Self::SecretServiceBearer { .. }
            | Self::OAuth { .. } => None,
        }
    }

    pub(crate) fn stored_prefix(&self) -> Option<&str> {
        match self {
            Self::SecretServiceHeader { prefix, .. } => Some(prefix),
            Self::None
            | Self::GhCli { .. }
            | Self::SecretServiceBearer { .. }
            | Self::OAuth { .. } => None,
        }
    }

    pub(crate) fn stored_oauth_registration(&self) -> Result<Option<String>, McpHostError> {
        match self {
            Self::OAuth { registration, .. } => Ok(Some(serde_json::to_string(registration)?)),
            Self::None
            | Self::GhCli { .. }
            | Self::SecretServiceBearer { .. }
            | Self::SecretServiceHeader { .. } => Ok(None),
        }
    }

    pub(crate) fn oauth_credential_id(&self) -> Option<&str> {
        match self {
            Self::OAuth { credential_id, .. } => Some(credential_id),
            Self::None
            | Self::GhCli { .. }
            | Self::SecretServiceBearer { .. }
            | Self::SecretServiceHeader { .. } => None,
        }
    }

    pub(crate) fn oauth_registration(&self) -> Option<&McpOAuthRegistration> {
        match self {
            Self::OAuth { registration, .. } => Some(registration),
            Self::None
            | Self::GhCli { .. }
            | Self::SecretServiceBearer { .. }
            | Self::SecretServiceHeader { .. } => None,
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

fn validate_credential_header(value: &str) -> Result<(), McpHostError> {
    let lower = value.to_ascii_lowercase();
    let valid = !value.is_empty()
        && value.is_ascii()
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || HEADER_TOKEN.contains(&byte))
        && !FORBIDDEN_CREDENTIAL_HEADERS.contains(&lower.as_str());
    if valid {
        Ok(())
    } else {
        Err(McpHostError::Invalid(
            "credential header must be an allowed RFC 9110 field name".to_owned(),
        ))
    }
}

fn validate_credential_prefix(value: &str) -> Result<(), McpHostError> {
    if value.len() <= MAX_CREDENTIAL_PREFIX_BYTES
        && value
            .bytes()
            .all(|byte| byte == b'\t' || matches!(byte, 0x20..=0x7e))
    {
        Ok(())
    } else {
        Err(McpHostError::Invalid(format!(
            "credential header prefix must be at most {MAX_CREDENTIAL_PREFIX_BYTES} printable ASCII bytes"
        )))
    }
}
