use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio_util::sync::CancellationToken;

use super::{SensitiveString, private_secret::PrivateSecretStore, secret::SecretService};
use crate::mcp::{
    McpAdapterError, McpCredentialError, McpHostError, McpOAuthError, validate_identity,
};

const SERVICE_SOURCE: &str = "Secret Service";
const PRIVATE_SOURCE: &str = "private Host OAuth store";
const MAX_BUNDLE_BYTES: usize = 768 * 1_024;
const MAX_CLIENT_CREDENTIAL_BYTES: usize = 64 * 1_024;
const MAX_CLIENT_VALUE_BYTES: usize = 16 * 1_024;

#[derive(Clone)]
pub(super) enum OAuthSecretStore {
    Service(SecretService),
    Private(PrivateSecretStore),
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct OAuthSecretBundle {
    schema_version: u32,
    pub(super) adapter_state: Value,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(super) pending_callback: Option<PendingCallback>,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct PendingCallback {
    pub(super) authorization_code: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(super) issuer: Option<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct PreRegisteredOAuthClient {
    schema_version: u32,
    pub(super) issuer: String,
    pub(super) client_id: String,
    #[serde(default)]
    pub(super) client_secret: Option<SensitiveString>,
}

impl OAuthSecretBundle {
    pub(super) const fn new(adapter_state: Value) -> Self {
        Self {
            schema_version: 1,
            adapter_state,
            pending_callback: None,
        }
    }
}

impl OAuthSecretStore {
    pub(super) fn service(executable: PathBuf) -> Self {
        Self::Service(SecretService::new(executable))
    }

    pub(super) const fn from_private(store: PrivateSecretStore) -> Self {
        Self::Private(store)
    }

    pub(super) async fn load(
        &self,
        credential_id: &str,
        cancellation: CancellationToken,
    ) -> Result<Option<OAuthSecretBundle>, McpHostError> {
        validate_identity("credential", credential_id)?;
        let Some(mut bytes) = self
            .load_bytes(credential_id, MAX_BUNDLE_BYTES, cancellation)
            .await?
        else {
            return Ok(None);
        };
        let decoded = serde_json::from_slice::<OAuthSecretBundle>(&bytes);
        bytes.fill(0);
        let bundle = decoded.map_err(|_| self.invalid_output())?;
        if bundle.schema_version != 1 {
            return Err(self.invalid_output());
        }
        Ok(Some(bundle))
    }

    pub(super) async fn store(
        &self,
        credential_id: &str,
        bundle: &OAuthSecretBundle,
        cancellation: CancellationToken,
    ) -> Result<(), McpHostError> {
        validate_identity("credential", credential_id)?;
        let mut encoded = serde_json::to_vec(bundle)?;
        if encoded.len() > MAX_BUNDLE_BYTES {
            encoded.fill(0);
            return Err(self.output_limit());
        }
        let result = match self {
            Self::Service(service) => service
                .store_bytes(credential_id, &encoded, cancellation)
                .await
                .map_err(credential_error),
            Self::Private(store) => {
                if cancellation.is_cancelled() {
                    Err(McpOAuthError::Cancelled.into())
                } else {
                    store.store(credential_id, encoded.clone()).await
                }
            }
        };
        encoded.fill(0);
        result
    }

    pub(super) async fn load_pre_registered_client(
        &self,
        credential_id: &str,
        cancellation: CancellationToken,
    ) -> Result<PreRegisteredOAuthClient, McpHostError> {
        validate_identity("OAuth client credential", credential_id)?;
        let Some(mut bytes) = self
            .load_bytes(credential_id, MAX_CLIENT_CREDENTIAL_BYTES, cancellation)
            .await?
        else {
            return Err(credential_error(McpCredentialError::Unavailable {
                source_name: self.source(),
                reference: format!("OAuth client credential `{credential_id}`"),
                status: "not found".to_owned(),
                guidance: "store schema-version 1 JSON with issuer, client_id, and optional client_secret in the configured Host credential facility".to_owned(),
            }));
        };
        let decoded = serde_json::from_slice::<PreRegisteredOAuthClient>(&bytes);
        bytes.fill(0);
        let mut client = decoded.map_err(|_| {
            McpOAuthError::Invalid(format!(
                "pre-registered OAuth credential '{credential_id}' must be schema-version 1 JSON with issuer, client_id, and optional client_secret"
            ))
        })?;
        if client.schema_version != 1
            || client.issuer.len() > MAX_CLIENT_VALUE_BYTES
            || client.client_id.is_empty()
            || client.client_id.len() > MAX_CLIENT_VALUE_BYTES
            || client
                .client_secret
                .as_ref()
                .is_some_and(|secret| secret.is_empty() || secret.len() > MAX_CLIENT_VALUE_BYTES)
        {
            return Err(McpOAuthError::Invalid(format!(
                "pre-registered OAuth credential '{credential_id}' has invalid or oversized fields"
            ))
            .into());
        }
        client.issuer = validate_issuer(&client.issuer).map_err(|reason| {
            McpOAuthError::Invalid(format!(
                "pre-registered OAuth credential '{credential_id}' has an invalid issuer: {reason}"
            ))
        })?;
        Ok(client)
    }

    async fn load_bytes(
        &self,
        credential_id: &str,
        limit: usize,
        cancellation: CancellationToken,
    ) -> Result<Option<Vec<u8>>, McpHostError> {
        if cancellation.is_cancelled() {
            return Err(McpOAuthError::Cancelled.into());
        }
        let output = match self {
            Self::Service(service) => service
                .lookup(credential_id, cancellation)
                .await
                .map_err(credential_error)?,
            Self::Private(store) => store.lookup(credential_id, limit).await?,
        };
        if output.as_ref().is_some_and(|bytes| bytes.len() > limit) {
            if let Some(mut bytes) = output {
                bytes.fill(0);
            }
            return Err(self.output_limit());
        }
        Ok(output)
    }

    const fn source(&self) -> &'static str {
        match self {
            Self::Service(_) => SERVICE_SOURCE,
            Self::Private(_) => PRIVATE_SOURCE,
        }
    }

    fn output_limit(&self) -> McpHostError {
        credential_error(McpCredentialError::OutputLimit(self.source()))
    }

    fn invalid_output(&self) -> McpHostError {
        credential_error(McpCredentialError::InvalidOutput(self.source()))
    }
}

pub(crate) fn validate_issuer(value: &str) -> Result<String, &'static str> {
    let issuer = url::Url::parse(value).map_err(|_| "it must be an absolute URL")?;
    let loopback_http = issuer.scheme() == "http"
        && issuer.host_str().is_some_and(|host| {
            host.eq_ignore_ascii_case("localhost")
                || host
                    .trim_matches(['[', ']'])
                    .parse::<std::net::IpAddr>()
                    .is_ok_and(|address| address.is_loopback())
        });
    if issuer.scheme() != "https" && !loopback_http {
        return Err("it must use HTTPS, except for loopback testing");
    }
    if !issuer.username().is_empty()
        || issuer.password().is_some()
        || issuer.query().is_some()
        || issuer.fragment().is_some()
    {
        return Err("it must not contain credentials, a query, or a fragment");
    }
    Ok(if issuer.path() == "/" {
        issuer.origin().ascii_serialization()
    } else {
        issuer.to_string()
    })
}

fn credential_error(error: McpCredentialError) -> McpHostError {
    McpAdapterError::Credential(error).into()
}
