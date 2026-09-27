pub use renoa_credential_relay_protocol::CredentialRelayKind as PluginCredentialKind;
use serde::{Deserialize, Serialize};

/// Progress events for the Host and every interaction surface. URLs are delivered
/// to the operator; credentials themselves never appear in these events.
#[derive(Debug, Deserialize, Serialize)]
#[serde(tag = "status", rename_all = "snake_case", deny_unknown_fields)]
pub enum PluginProgress {
    AuthorizationRequired(PluginAuthorizationRequired),
    CredentialRequired(PluginCredentialRequired),
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct PluginAuthorizationRequired {
    pub connection: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub display_name: Option<String>,
    pub authorization_url: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expires_at_ms: Option<i64>,
    pub message: String,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct PluginCredentialRequired {
    pub credential: String,
    pub credential_kind: PluginCredentialKind,
    pub setup_url: String,
    pub expires_at_ms: i64,
    pub message: String,
}
