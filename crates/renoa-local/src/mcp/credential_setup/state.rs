use renoa_credential_relay_protocol::{CredentialRelayId, CredentialRelayKind};
use serde::{Deserialize, Serialize};

use crate::mcp::{
    McpCredentialError, McpHostError, hex_sha256,
    oauth::{PrivateSecretStore, SensitiveString},
};

const MAX_STATE_BYTES: usize = 4 * 1024;

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct CredentialSetupState {
    schema_version: u32,
    pub(super) relay_id: CredentialRelayId,
    pub(super) credential_id: String,
    pub(super) kind: CredentialRelayKind,
    pub(super) key: SensitiveString,
    pub(super) capability: SensitiveString,
    pub(super) expires_at_ms: i64,
}

impl CredentialSetupState {
    pub(super) fn new(
        credential_id: String,
        kind: CredentialRelayKind,
    ) -> Result<Self, McpHostError> {
        Ok(Self {
            schema_version: 1,
            relay_id: CredentialRelayId::new(),
            credential_id,
            kind,
            key: random_secret()?,
            capability: random_secret()?,
            expires_at_ms: 0,
        })
    }
}

#[derive(Clone)]
pub(super) struct CredentialSetupStateStore {
    store: PrivateSecretStore,
}

impl CredentialSetupStateStore {
    pub(super) fn initialize(directory: std::path::PathBuf) -> Result<Self, McpHostError> {
        Ok(Self {
            store: PrivateSecretStore::initialize(directory)?,
        })
    }

    pub(super) async fn load(
        &self,
        state_id: &str,
    ) -> Result<Option<CredentialSetupState>, McpHostError> {
        let Some(mut bytes) = self.store.lookup(state_id, MAX_STATE_BYTES).await? else {
            return Ok(None);
        };
        let decoded = serde_json::from_slice::<CredentialSetupState>(&bytes);
        bytes.fill(0);
        let state = decoded.map_err(|_| McpCredentialError::SetupInvalid)?;
        if state.schema_version != 1
            || state.credential_id.is_empty()
            || state.key.expose().len() != 64
            || state.capability.expose().len() != 64
        {
            return Err(McpCredentialError::SetupInvalid.into());
        }
        Ok(Some(state))
    }

    pub(super) async fn store(
        &self,
        state_id: &str,
        state: &CredentialSetupState,
    ) -> Result<(), McpHostError> {
        let mut bytes = serde_json::to_vec(state)?;
        if bytes.len() > MAX_STATE_BYTES {
            bytes.fill(0);
            return Err(McpCredentialError::SetupInvalid.into());
        }
        let result = self.store.store(state_id, bytes.clone()).await;
        bytes.fill(0);
        result
    }

    pub(super) async fn delete(&self, state_id: &str) -> Result<(), McpHostError> {
        self.store.delete(state_id).await
    }
}

pub(super) fn state_id(
    operation_id: &str,
    credential_id: &str,
    kind: CredentialRelayKind,
) -> String {
    let mut identity = b"renoa credential setup operation v1\0".to_vec();
    identity.extend_from_slice(operation_id.as_bytes());
    identity.push(0);
    identity.extend_from_slice(credential_id.as_bytes());
    identity.push(0);
    identity.extend_from_slice(match kind {
        CredentialRelayKind::ApiToken => b"api_token",
        CredentialRelayKind::OAuthClient => b"oauth_client",
    });
    format!("setup.{}", hex_sha256(&identity))
}

fn random_secret() -> Result<SensitiveString, McpHostError> {
    let mut bytes = [0_u8; 32];
    getrandom::fill(&mut bytes).map_err(|error| {
        McpCredentialError::SetupUnavailable(format!("secure randomness failed: {error}"))
    })?;
    let encoded = hex(&bytes);
    bytes.fill(0);
    serde_json::from_value(serde_json::Value::String(encoded))
        .map_err(|_| McpCredentialError::SetupInvalid.into())
}

fn hex(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut encoded = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        encoded.push(char::from(HEX[usize::from(byte >> 4)]));
        encoded.push(char::from(HEX[usize::from(byte & 0x0f)]));
    }
    encoded
}
