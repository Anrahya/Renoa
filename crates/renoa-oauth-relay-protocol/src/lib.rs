//! Shared HTTP values for Renoa's short-lived OAuth callback relay.

use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Current callback-relay HTTP contract revision.
pub const OAUTH_RELAY_VERSION: u32 = 1;

/// Fixed provider-facing callback path at the configured Renoa HTTPS origin.
pub const OAUTH_CALLBACK_PATH: &str = "/v1/oauth/callback";

/// Device-authenticated relay collection path.
pub const OAUTH_RELAYS_PATH: &str = "/v1/oauth/relays";

/// Header carrying the enrolled Renoa device identity.
pub const DEVICE_ID_HEADER: &str = "x-renoa-device-id";

/// One client-selected, stable callback relay identity.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct OAuthRelayId(Uuid);

impl OAuthRelayId {
    /// Creates a fresh relay identity.
    #[must_use]
    pub fn new() -> Self {
        Self(Uuid::new_v4())
    }

    /// Reconstructs a validated relay identity.
    #[must_use]
    pub const fn from_uuid(value: Uuid) -> Self {
        Self(value)
    }

    /// Returns the underlying UUID.
    #[must_use]
    pub const fn as_uuid(self) -> Uuid {
        self.0
    }
}

impl Default for OAuthRelayId {
    fn default() -> Self {
        Self::new()
    }
}

impl std::fmt::Display for OAuthRelayId {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.0.fmt(formatter)
    }
}

impl std::str::FromStr for OAuthRelayId {
    type Err = uuid::Error;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        value.parse().map(Self)
    }
}

/// Idempotent request to reserve one callback relay.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CreateOAuthRelayRequest {
    pub version: u32,
    pub relay_id: OAuthRelayId,
    pub state_digest: String,
}

/// Durable relay reservation returned to the execution Host.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CreateOAuthRelayResponse {
    pub version: u32,
    pub relay_id: OAuthRelayId,
    pub redirect_uri: String,
    pub expires_at_ms: i64,
}

/// Current durable provider-callback state.
///
/// Deliberately does not implement `Debug`: an authorized value contains a
/// short-lived authorization code.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case", deny_unknown_fields)]
pub enum OAuthRelayStatus {
    Pending {
        version: u32,
    },
    Authorized {
        version: u32,
        authorization_code: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        issuer: Option<String>,
    },
    Rejected {
        version: u32,
        error: String,
    },
    Acknowledged {
        version: u32,
    },
}

impl OAuthRelayStatus {
    /// Returns the contract revision carried by this status.
    #[must_use]
    pub const fn version(&self) -> u32 {
        match self {
            Self::Pending { version }
            | Self::Authorized { version, .. }
            | Self::Rejected { version, .. }
            | Self::Acknowledged { version } => *version,
        }
    }
}

/// Idempotent request sent after the Host has durably retained the callback.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AcknowledgeOAuthRelayRequest {
    pub version: u32,
}

/// Successful callback acknowledgement.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AcknowledgeOAuthRelayResponse {
    pub version: u32,
}

/// Safe, credential-free HTTP error body.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct OAuthRelayErrorResponse {
    pub code: String,
    pub message: String,
}
