//! What a device key signs, and the bodies a computer signs in by a code with
//! (`docs/identity-v0.md`, "Signing in a computer from a phone"). The
//! coordinator refuses a request body with a field it does not know; a client
//! reads an answer with fields it does not know, so the coordinator may add
//! them.

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::{DeviceCode, DeviceCredentials, DeviceId, DeviceRequestProof};

/// Where a computer asks to sign in.
pub const DEVICE_REQUESTS_PATH: &str = "/v1/identity/device-requests";
/// Where a computer collects the credential once an app approved its code.
pub const DEVICE_REQUEST_COLLECT_PATH: &str = "/v1/identity/device-requests/collect";

/// What a connection signs with its device key to answer `device_challenge`.
#[must_use]
pub fn device_proof_message(device_id: DeviceId, nonce: &str) -> Vec<u8> {
    format!("renoa device proof v1\n{device_id}\n{nonce}").into_bytes()
}

/// What a computer signs to ask for, and then to collect, a sign-in another
/// of its owner's apps approves. The computer chose `request_id`.
#[must_use]
pub fn device_request_message(request_id: Uuid) -> Vec<u8> {
    format!("renoa device request v1\n{request_id}").into_bytes()
}

/// A computer's request to sign in, sent to [`DEVICE_REQUESTS_PATH`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DeviceRequest {
    pub request_id: Uuid,
    pub surface: String,
    pub device: RequestingDevice,
    /// The device key's signature over [`device_request_message`].
    pub device_proof: DeviceRequestProof,
}

/// The key and name a computer asks to sign in with.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RequestingDevice {
    /// The uncompressed P-256 point, base64url without padding.
    pub public_key: String,
    pub label: String,
}

/// The code the computer shows until an app approves it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DeviceRequestCode {
    pub code: DeviceCode,
    pub expires_at_ms: i64,
}

/// A computer's collection, sent to [`DEVICE_REQUEST_COLLECT_PATH`] with the
/// same proof it asked with.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CollectDeviceRequest {
    pub request_id: Uuid,
    pub device_proof: DeviceRequestProof,
}

/// The `202` answer while no app has approved the request.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AwaitingApproval {
    pub expires_at_ms: i64,
}

/// The `200` answer once an app approved the request.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ApprovedCredentials {
    pub credentials: DeviceCredentials,
}

/// The body of every refusal on the identity routes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct IdentityRefusal {
    pub code: String,
    pub message: String,
}
