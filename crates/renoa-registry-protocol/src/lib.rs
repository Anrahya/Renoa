//! Transport-neutral wire contract for Renoa's shared plugin registry.

use std::{fmt, str::FromStr};

use serde::{Deserialize, Serialize};
use thiserror::Error;
use uuid::Uuid;

pub const API_VERSION: &str = "v1";
pub const PACKAGE_MEDIA_TYPE: &str = "application/vnd.renoa.agent-plugin.v1+tar";
pub const ARCHIVE_DIGEST_HEADER: &str = "x-renoa-archive-sha256";
pub const MAX_PACKAGE_ARCHIVE_BYTES: u64 = 144 * 1_024 * 1_024;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct RegistryId(Uuid);

impl RegistryId {
    #[must_use]
    pub fn new() -> Self {
        Self(Uuid::new_v4())
    }

    #[must_use]
    pub const fn from_uuid(value: Uuid) -> Self {
        Self(value)
    }

    #[must_use]
    pub const fn as_uuid(self) -> Uuid {
        self.0
    }
}

impl Default for RegistryId {
    fn default() -> Self {
        Self::new()
    }
}

impl fmt::Display for RegistryId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(formatter)
    }
}

impl FromStr for RegistryId {
    type Err = uuid::Error;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        value.parse().map(Self)
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize)]
#[serde(transparent)]
pub struct Sha256Digest(String);

impl Sha256Digest {
    /// Parses one canonical lowercase SHA-256 digest.
    ///
    /// # Errors
    ///
    /// Returns when the value is not exactly 64 lowercase hexadecimal characters.
    pub fn parse(value: impl Into<String>) -> Result<Self, ProtocolError> {
        let value = value.into();
        if value.len() != 64
            || !value
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        {
            return Err(ProtocolError::InvalidDigest(value));
        }
        Ok(Self(value))
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl<'de> Deserialize<'de> for Sha256Digest {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        Self::parse(value).map_err(serde::de::Error::custom)
    }
}

impl fmt::Display for Sha256Digest {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(formatter)
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RegistryStatus {
    registry_id: RegistryId,
    current_revision: u64,
}

impl RegistryStatus {
    #[must_use]
    pub const fn new(registry_id: RegistryId, current_revision: u64) -> Self {
        Self {
            registry_id,
            current_revision,
        }
    }

    #[must_use]
    pub const fn registry_id(&self) -> RegistryId {
        self.registry_id
    }

    #[must_use]
    pub const fn current_revision(&self) -> u64 {
        self.current_revision
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "PackageRecordWire")]
pub struct PackageRecord {
    revision: u64,
    package_digest: Sha256Digest,
    archive_digest: Sha256Digest,
    archive_bytes: u64,
}

impl PackageRecord {
    /// Creates one ordered immutable package record.
    ///
    /// # Errors
    ///
    /// Returns when the revision or archive length is zero.
    pub fn new(
        revision: u64,
        package_digest: Sha256Digest,
        archive_digest: Sha256Digest,
        archive_bytes: u64,
    ) -> Result<Self, ProtocolError> {
        if revision == 0 {
            return Err(ProtocolError::ZeroRevision);
        }
        if archive_bytes == 0 {
            return Err(ProtocolError::EmptyArchive);
        }
        Ok(Self {
            revision,
            package_digest,
            archive_digest,
            archive_bytes,
        })
    }

    #[must_use]
    pub const fn revision(&self) -> u64 {
        self.revision
    }

    #[must_use]
    pub const fn package_digest(&self) -> &Sha256Digest {
        &self.package_digest
    }

    #[must_use]
    pub const fn archive_digest(&self) -> &Sha256Digest {
        &self.archive_digest
    }

    #[must_use]
    pub const fn archive_bytes(&self) -> u64 {
        self.archive_bytes
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct PackageRecordWire {
    revision: u64,
    package_digest: Sha256Digest,
    archive_digest: Sha256Digest,
    archive_bytes: u64,
}

impl TryFrom<PackageRecordWire> for PackageRecord {
    type Error = ProtocolError;

    fn try_from(value: PackageRecordWire) -> Result<Self, Self::Error> {
        Self::new(
            value.revision,
            value.package_digest,
            value.archive_digest,
            value.archive_bytes,
        )
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RegistryChanges {
    registry_id: RegistryId,
    current_revision: u64,
    packages: Vec<PackageRecord>,
}

impl RegistryChanges {
    #[must_use]
    pub fn new(
        registry_id: RegistryId,
        current_revision: u64,
        packages: Vec<PackageRecord>,
    ) -> Self {
        Self {
            registry_id,
            current_revision,
            packages,
        }
    }

    #[must_use]
    pub const fn registry_id(&self) -> RegistryId {
        self.registry_id
    }

    #[must_use]
    pub const fn current_revision(&self) -> u64 {
        self.current_revision
    }

    #[must_use]
    pub fn packages(&self) -> &[PackageRecord] {
        &self.packages
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PublishDisposition {
    Published,
    Existing,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "PublishResultWire")]
pub struct PublishResult {
    registry_id: RegistryId,
    revision: u64,
    disposition: PublishDisposition,
}

impl PublishResult {
    /// Creates one publication result with a real registry revision.
    ///
    /// # Errors
    ///
    /// Returns when the revision is zero.
    pub const fn new(
        registry_id: RegistryId,
        revision: u64,
        disposition: PublishDisposition,
    ) -> Result<Self, ProtocolError> {
        if revision == 0 {
            return Err(ProtocolError::ZeroRevision);
        }
        Ok(Self {
            registry_id,
            revision,
            disposition,
        })
    }

    #[must_use]
    pub const fn registry_id(&self) -> RegistryId {
        self.registry_id
    }

    #[must_use]
    pub const fn revision(&self) -> u64 {
        self.revision
    }

    #[must_use]
    pub const fn disposition(&self) -> PublishDisposition {
        self.disposition
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct PublishResultWire {
    registry_id: RegistryId,
    revision: u64,
    disposition: PublishDisposition,
}

impl TryFrom<PublishResultWire> for PublishResult {
    type Error = ProtocolError;

    fn try_from(value: PublishResultWire) -> Result<Self, Self::Error> {
        Self::new(value.registry_id, value.revision, value.disposition)
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ErrorResponse {
    code: String,
    message: String,
}

impl ErrorResponse {
    #[must_use]
    pub fn new(code: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            code: code.into(),
            message: message.into(),
        }
    }

    #[must_use]
    pub fn code(&self) -> &str {
        &self.code
    }

    #[must_use]
    pub fn message(&self) -> &str {
        &self.message
    }
}

#[derive(Debug, Error)]
pub enum ProtocolError {
    #[error("digest must be exactly 64 lowercase hexadecimal characters, found `{0}`")]
    InvalidDigest(String),
    #[error("package revision must be greater than zero")]
    ZeroRevision,
    #[error("package archive must not be empty")]
    EmptyArchive,
}

#[cfg(test)]
mod tests {
    use super::{PackageRecord, PublishResult, Sha256Digest};

    #[test]
    fn digests_are_canonical_lowercase_sha256() {
        let valid = "a".repeat(64);
        assert_eq!(
            Sha256Digest::parse(valid.clone())
                .expect("valid digest")
                .as_str(),
            valid
        );
        for invalid in ["a".repeat(63), "A".repeat(64), "g".repeat(64)] {
            assert!(Sha256Digest::parse(invalid).is_err());
        }
    }

    #[test]
    fn package_records_require_real_revisions_and_archives() {
        let digest = Sha256Digest::parse("a".repeat(64)).expect("valid digest");
        assert!(PackageRecord::new(0, digest.clone(), digest.clone(), 1).is_err());
        assert!(PackageRecord::new(1, digest.clone(), digest, 0).is_err());
    }

    #[test]
    fn hostile_wire_records_cannot_bypass_constructor_invariants() {
        let digest = "a".repeat(64);
        let package = format!(
            r#"{{"revision":0,"package_digest":"{digest}","archive_digest":"{digest}","archive_bytes":1}}"#
        );
        assert!(serde_json::from_str::<PackageRecord>(&package).is_err());

        let publication = format!(
            r#"{{"registry_id":"{}","revision":0,"disposition":"published"}}"#,
            super::RegistryId::new()
        );
        assert!(serde_json::from_str::<PublishResult>(&publication).is_err());

        let unknown = format!(
            r#"{{"revision":1,"package_digest":"{digest}","archive_digest":"{digest}","archive_bytes":1,"ignored":true}}"#
        );
        assert!(serde_json::from_str::<PackageRecord>(&unknown).is_err());
    }
}
