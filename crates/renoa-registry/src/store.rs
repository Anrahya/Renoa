use std::{
    collections::BTreeSet,
    fs::File,
    path::{Path, PathBuf},
    sync::Arc,
};

use renoa_registry_protocol::{
    MAX_PACKAGE_ARCHIVE_BYTES, PackageRecord, PublishDisposition, PublishResult, RegistryChanges,
    RegistryId, RegistryStatus, Sha256Digest,
};
use rusqlite::{OptionalExtension as _, TransactionBehavior, params};
use thiserror::Error;

use crate::{blob, schema};

const DATABASE_FILE: &str = "registry.sqlite3";
const BLOBS_DIRECTORY: &str = "blobs";
const STAGING_DIRECTORY: &str = "staging";
const LOCK_FILE: &str = "registry.lock";

#[derive(Clone)]
pub(crate) struct RegistryStore {
    _lock: Arc<File>,
    database: Arc<PathBuf>,
    blobs: Arc<PathBuf>,
    staging: Arc<PathBuf>,
    registry_id: RegistryId,
}

#[derive(Debug, Error)]
pub enum RegistryError {
    #[error("registry I/O failed: {0}")]
    Io(#[from] std::io::Error),
    #[error("registry database failed: {0}")]
    Database(#[from] rusqlite::Error),
    #[error("invalid registry state: {0}")]
    InvalidState(String),
    #[error("invalid registry request: {0}")]
    InvalidRequest(String),
    #[error("registry package was not found")]
    NotFound,
    #[error("registry package conflicts with its existing content: {0}")]
    Conflict(String),
    #[error("registry worker failed: {0}")]
    Worker(#[from] tokio::task::JoinError),
}

impl RegistryStore {
    pub(crate) fn open(root: &Path) -> Result<Self, RegistryError> {
        let root = blob::initialize_directory(root)?;
        let lock = blob::acquire_lock(&root.join(LOCK_FILE))?;
        let blobs = blob::initialize_directory(&root.join(BLOBS_DIRECTORY))?;
        let staging = blob::initialize_directory(&root.join(STAGING_DIRECTORY))?;
        let database = root.join(DATABASE_FILE);
        let registry_id = schema::initialize(&database)?;
        let store = Self {
            _lock: Arc::new(lock),
            database: Arc::new(database),
            blobs: Arc::new(blobs),
            staging: Arc::new(staging),
            registry_id,
        };
        blob::clean_staging(&store.staging)?;
        let referenced = store.verify_records()?;
        blob::clean_unreferenced(&store.blobs, &referenced)?;
        Ok(store)
    }

    pub(crate) fn status(&self) -> Result<RegistryStatus, RegistryError> {
        let connection = schema::open_verified(&self.database)?;
        let current = connection.query_row(
            "SELECT COALESCE(MAX(revision), 0) FROM packages",
            [],
            |row| row.get::<_, i64>(0).and_then(sql_u64),
        )?;
        Ok(RegistryStatus::new(self.registry_id, current))
    }

    pub(crate) fn changes(
        &self,
        after: u64,
        limit: usize,
    ) -> Result<RegistryChanges, RegistryError> {
        if !(1..=256).contains(&limit) {
            return Err(RegistryError::InvalidRequest(
                "change limit must be between 1 and 256".to_owned(),
            ));
        }
        let after = i64::try_from(after).map_err(|_| {
            RegistryError::InvalidRequest("change cursor exceeds SQLite range".to_owned())
        })?;
        let limit = i64::try_from(limit).map_err(|_| {
            RegistryError::InvalidRequest("change limit exceeds SQLite range".to_owned())
        })?;
        let mut connection = schema::open_verified(&self.database)?;
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Deferred)?;
        let current = transaction.query_row(
            "SELECT COALESCE(MAX(revision), 0) FROM packages",
            [],
            |row| row.get::<_, i64>(0).and_then(sql_u64),
        )?;
        let mut statement = transaction.prepare(
            "SELECT revision, package_digest, archive_digest, archive_bytes
             FROM packages WHERE revision > ?1 ORDER BY revision LIMIT ?2",
        )?;
        let packages = statement
            .query_map(params![after, limit], record_from_row)?
            .collect::<Result<Vec<_>, _>>()?;
        drop(statement);
        transaction.commit()?;
        Ok(RegistryChanges::new(self.registry_id, current, packages))
    }

    pub(crate) fn package(&self, digest: &Sha256Digest) -> Result<PackageRecord, RegistryError> {
        let connection = schema::open_verified(&self.database)?;
        connection
            .query_row(
                "SELECT revision, package_digest, archive_digest, archive_bytes
                 FROM packages WHERE package_digest = ?1",
                [digest.as_str()],
                record_from_row,
            )
            .optional()?
            .ok_or(RegistryError::NotFound)
    }

    pub(crate) fn verified_blob(&self, record: &PackageRecord) -> Result<PathBuf, RegistryError> {
        let path = self.blobs.join(record.archive_digest().as_str());
        blob::verify_file(&path, record.archive_digest(), record.archive_bytes())?;
        Ok(path)
    }

    pub(crate) fn staging_path(&self) -> PathBuf {
        self.staging
            .join(format!("upload-{}.tar", uuid::Uuid::new_v4()))
    }

    pub(crate) fn publish(
        &self,
        staging: &Path,
        package_digest: &Sha256Digest,
        archive_digest: &Sha256Digest,
        archive_bytes: u64,
    ) -> Result<PublishResult, RegistryError> {
        if archive_bytes == 0 || archive_bytes > MAX_PACKAGE_ARCHIVE_BYTES {
            return Err(RegistryError::InvalidRequest(format!(
                "archive size must be between 1 and {MAX_PACKAGE_ARCHIVE_BYTES} bytes"
            )));
        }
        blob::verify_file(staging, archive_digest, archive_bytes)?;
        let mut connection = schema::open_verified(&self.database)?;
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        if let Some((stored_archive, stored_bytes, revision)) = transaction
            .query_row(
                "SELECT archive_digest, archive_bytes, revision
                 FROM packages WHERE package_digest = ?1",
                [package_digest.as_str()],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        sql_u64(row.get::<_, i64>(1)?)?,
                        sql_u64(row.get::<_, i64>(2)?)?,
                    ))
                },
            )
            .optional()?
        {
            if stored_archive != archive_digest.as_str() || stored_bytes != archive_bytes {
                return Err(RegistryError::Conflict(package_digest.to_string()));
            }
            self.verified_blob(
                &PackageRecord::new(
                    revision,
                    package_digest.clone(),
                    archive_digest.clone(),
                    archive_bytes,
                )
                .map_err(|error| RegistryError::InvalidState(error.to_string()))?,
            )?;
            blob::remove_staging(staging)?;
            transaction.commit()?;
            return PublishResult::new(self.registry_id, revision, PublishDisposition::Existing)
                .map_err(|error| RegistryError::InvalidState(error.to_string()));
        }
        blob::publish(staging, &self.blobs, archive_digest, archive_bytes)?;
        let revision = transaction.query_row(
            "SELECT COALESCE(MAX(revision), 0) + 1 FROM packages",
            [],
            |row| row.get::<_, i64>(0).and_then(sql_u64),
        )?;
        let stored_archive_bytes = sql_i64(archive_bytes)?;
        let stored_revision = sql_i64(revision)?;
        transaction.execute(
            "INSERT INTO packages(package_digest, archive_digest, archive_bytes, revision)
             VALUES (?1, ?2, ?3, ?4)",
            params![
                package_digest.as_str(),
                archive_digest.as_str(),
                stored_archive_bytes,
                stored_revision
            ],
        )?;
        transaction.commit()?;
        PublishResult::new(self.registry_id, revision, PublishDisposition::Published)
            .map_err(|error| RegistryError::InvalidState(error.to_string()))
    }

    pub(crate) fn discard_staging(path: &Path) -> Result<(), RegistryError> {
        blob::remove_staging(path)
    }

    fn verify_records(&self) -> Result<BTreeSet<String>, RegistryError> {
        let status = self.status()?;
        let mut after = 0;
        let mut referenced = BTreeSet::new();
        while after < status.current_revision() {
            let changes = self.changes(after, 256)?;
            if changes.packages().is_empty() {
                return Err(RegistryError::InvalidState(
                    "registry revision log contains a gap".to_owned(),
                ));
            }
            for record in changes.packages() {
                if record.revision() != after + 1 {
                    return Err(RegistryError::InvalidState(
                        "registry revision log is not contiguous".to_owned(),
                    ));
                }
                self.verified_blob(record)?;
                referenced.insert(record.archive_digest().to_string());
                after = record.revision();
            }
        }
        Ok(referenced)
    }
}

fn record_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<PackageRecord> {
    let revision = sql_u64(row.get::<_, i64>(0)?)?;
    let package = row.get::<_, String>(1)?;
    let archive = row.get::<_, String>(2)?;
    let bytes = sql_u64(row.get::<_, i64>(3)?)?;
    let package = Sha256Digest::parse(package).map_err(to_sql_error)?;
    let archive = Sha256Digest::parse(archive).map_err(to_sql_error)?;
    PackageRecord::new(revision, package, archive, bytes).map_err(to_sql_error)
}

fn sql_u64(value: i64) -> rusqlite::Result<u64> {
    u64::try_from(value).map_err(to_sql_error)
}

fn sql_i64(value: u64) -> Result<i64, RegistryError> {
    i64::try_from(value).map_err(|_| {
        RegistryError::InvalidState("registry integer exceeds SQLite range".to_owned())
    })
}

fn to_sql_error(error: impl std::error::Error + Send + Sync + 'static) -> rusqlite::Error {
    rusqlite::Error::FromSqlConversionFailure(0, rusqlite::types::Type::Text, Box::new(error))
}
