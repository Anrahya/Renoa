//! Plugin setup notices as the frames carry them: sealed to one device's key,
//! opaque to the coordinator.

use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::DeviceId;

/// An uncompressed SEC1 P-256 point: the tag 0x04 and two 32-byte coordinates.
const PUBLIC_KEY_BYTES: usize = 65;

/// A device's key for opening the notices sealed to it: an uncompressed SEC1
/// P-256 public key, base64url without padding.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct NoticeKey(String);

impl NoticeKey {
    #[must_use]
    pub fn new(encoded: impl Into<String>) -> Self {
        Self(encoded.into())
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// The key's point, when it is one: 65 bytes starting 0x04. Whether the
    /// point is on the curve is learned when a node agrees with it.
    #[must_use]
    pub fn point(&self) -> Option<[u8; PUBLIC_KEY_BYTES]> {
        let bytes = URL_SAFE_NO_PAD.decode(&self.0).ok()?;
        let point: [u8; PUBLIC_KEY_BYTES] = bytes.try_into().ok()?;
        (point[0] == 0x04).then_some(point)
    }
}

/// A device a node may seal a notice to.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeviceNoticeKey {
    pub device_id: DeviceId,
    pub public_key: NoticeKey,
}

/// A notice a node puts for one device.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SealedNotice {
    pub notice_id: Uuid,
    pub device_id: DeviceId,
    /// The sealed envelope, base64url without padding.
    pub sealed: String,
    pub expires_at_ms: i64,
}

/// A notice as its device lists it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HeldNotice {
    pub notice_id: Uuid,
    pub sealed: String,
    pub expires_at_ms: i64,
}
