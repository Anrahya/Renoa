//! Shared HTTP values for end-to-end encrypted Host credential intake.

use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Current credential-relay HTTP contract revision.
pub const CREDENTIAL_RELAY_VERSION: u32 = 1;

/// Device-authenticated credential-relay collection path.
pub const CREDENTIAL_RELAYS_PATH: &str = "/v1/credential-relays";

/// Public, static browser helper used by a capability-bearing setup page.
pub const CREDENTIAL_SETUP_SCRIPT_PATH: &str = "/v1/credential-setup.js";

/// Header carrying an enrolled Renoa device identity.
pub const DEVICE_ID_HEADER: &str = "x-renoa-device-id";

/// One client-selected, stable credential intake identity.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct CredentialRelayId(Uuid);

impl CredentialRelayId {
    /// Creates a fresh relay identity.
    #[must_use]
    pub fn new() -> Self {
        Self(Uuid::new_v4())
    }

    /// Reconstructs a relay identity.
    #[must_use]
    pub const fn from_uuid(value: Uuid) -> Self {
        Self(value)
    }
}

impl Default for CredentialRelayId {
    fn default() -> Self {
        Self::new()
    }
}

impl std::fmt::Display for CredentialRelayId {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.0.fmt(formatter)
    }
}

impl std::str::FromStr for CredentialRelayId {
    type Err = uuid::Error;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        value.parse().map(Self)
    }
}

/// The exact secret shape collected by the trusted browser page.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CredentialRelayKind {
    ApiToken,
    OAuthClient,
}

/// Idempotent, device-authenticated relay reservation.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CreateCredentialRelayRequest {
    pub version: u32,
    pub relay_id: CredentialRelayId,
    pub credential_id: String,
    pub kind: CredentialRelayKind,
    pub capability_digest: String,
}

/// Durable reservation returned to the execution Host.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CreateCredentialRelayResponse {
    pub version: u32,
    pub relay_id: CredentialRelayId,
    pub expires_at_ms: i64,
}

/// Non-secret metadata used to render the browser form.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CredentialRelayForm {
    pub version: u32,
    pub relay_id: CredentialRelayId,
    pub credential_id: String,
    pub kind: CredentialRelayKind,
    pub expires_at_ms: i64,
}

/// Browser submission. Deliberately not `Debug`: every field is sensitive.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SubmitCredentialRelayRequest {
    pub version: u32,
    pub capability: String,
    pub nonce: String,
    pub ciphertext: String,
}

/// Successful, idempotent browser submission.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SubmitCredentialRelayResponse {
    pub version: u32,
}

/// Durable encrypted relay state. Deliberately not `Debug` because submitted
/// ciphertext and nonces should not enter incidental logs.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case", deny_unknown_fields)]
pub enum CredentialRelayStatus {
    Pending {
        version: u32,
    },
    Submitted {
        version: u32,
        nonce: String,
        ciphertext: String,
    },
    Acknowledged {
        version: u32,
    },
}

impl CredentialRelayStatus {
    /// Returns the contract revision carried by this status.
    #[must_use]
    pub const fn version(&self) -> u32 {
        match self {
            Self::Pending { version }
            | Self::Submitted { version, .. }
            | Self::Acknowledged { version } => *version,
        }
    }
}

/// Sent only after the Host has privately persisted and validated the secret.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AcknowledgeCredentialRelayRequest {
    pub version: u32,
}

/// Successful credential acknowledgement.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AcknowledgeCredentialRelayResponse {
    pub version: u32,
}

/// Safe, credential-free HTTP error body.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CredentialRelayErrorResponse {
    pub code: String,
    pub message: String,
}
