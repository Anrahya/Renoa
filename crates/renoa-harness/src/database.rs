use std::{
    ffi::OsString,
    fs::{File, OpenOptions},
    path::{Path, PathBuf},
    sync::Arc,
};

use rusqlite::Connection;

use crate::{HarnessError, schema::open_connection};

/// Keeps the database identity and its process-exclusive sidecar lock alive.
pub(crate) struct DatabaseLease {
    path: PathBuf,
    database_file: File,
    lock_path: PathBuf,
    owner_lock: File,
}

impl DatabaseLease {
    pub(crate) fn acquire(path: &Path) -> Result<Arc<Self>, HarnessError> {
        require_identity_support()?;
        let database_file = OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(path)
            .map_err(|error| HarnessError::Store(format!("open {}: {error}", path.display())))?;
        let path = path
            .canonicalize()
            .map_err(|error| HarnessError::Store(format!("resolve {}: {error}", path.display())))?;
        reject_database_links(&database_file, &path)?;

        let lock_path = lock_path(&path);
        let owner_lock = OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(&lock_path)
            .map_err(|error| {
                HarnessError::Store(format!("open {}: {error}", lock_path.display()))
            })?;
        owner_lock.try_lock().map_err(|error| match error {
            std::fs::TryLockError::WouldBlock => {
                HarnessError::AlreadyRunning { path: path.clone() }
            }
            std::fs::TryLockError::Error(error) => {
                HarnessError::Store(format!("lock {}: {error}", lock_path.display()))
            }
        })?;

        let lease = Arc::new(Self {
            path,
            database_file,
            lock_path,
            owner_lock,
        });
        lease.validate_identity()?;
        Ok(lease)
    }

    pub(crate) fn connection(&self) -> Result<Connection, HarnessError> {
        self.validate_identity()?;
        let connection = open_connection(&self.path)?;
        self.validate_identity()?;
        Ok(connection)
    }

    fn validate_identity(&self) -> Result<(), HarnessError> {
        validate_file_identity(&self.database_file, &self.path, true)?;
        validate_file_identity(&self.owner_lock, &self.lock_path, false)
    }
}

fn lock_path(database: &Path) -> PathBuf {
    let mut value: OsString = database.as_os_str().to_owned();
    value.push(".lock");
    PathBuf::from(value)
}

fn reject_database_links(database_file: &File, path: &Path) -> Result<(), HarnessError> {
    let metadata = database_file
        .metadata()
        .map_err(|error| HarnessError::Store(format!("inspect {}: {error}", path.display())))?;
    if link_count(&metadata) == 1 {
        Ok(())
    } else {
        Err(HarnessError::UnsupportedDatabaseAlias {
            path: path.to_owned(),
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct FileIdentity {
    first: u64,
    second: u64,
}

fn validate_file_identity(
    open_file: &File,
    path: &Path,
    reject_links: bool,
) -> Result<(), HarnessError> {
    let open_metadata = open_file.metadata().map_err(|error| {
        HarnessError::Store(format!("inspect open {}: {error}", path.display()))
    })?;
    let path_metadata = path
        .metadata()
        .map_err(|error| HarnessError::Store(format!("inspect {}: {error}", path.display())))?;
    if reject_links && link_count(&open_metadata) != 1 {
        return Err(HarnessError::UnsupportedDatabaseAlias {
            path: path.to_owned(),
        });
    }
    if file_identity(&open_metadata) != file_identity(&path_metadata) {
        return Err(HarnessError::Store(format!(
            "{} changed while the harness owned it",
            path.display()
        )));
    }
    Ok(())
}

#[cfg(unix)]
fn file_identity(metadata: &std::fs::Metadata) -> FileIdentity {
    use std::os::unix::fs::MetadataExt;

    FileIdentity {
        first: metadata.dev(),
        second: metadata.ino(),
    }
}

#[cfg(unix)]
fn link_count(metadata: &std::fs::Metadata) -> u64 {
    use std::os::unix::fs::MetadataExt;

    metadata.nlink()
}

#[cfg(unix)]
#[allow(
    clippy::unnecessary_wraps,
    reason = "the non-Unix implementation fails closed with the same signature"
)]
fn require_identity_support() -> Result<(), HarnessError> {
    Ok(())
}

#[cfg(not(unix))]
fn require_identity_support() -> Result<(), HarnessError> {
    Err(HarnessError::Store(
        "the durable harness currently requires Unix file identity support".to_owned(),
    ))
}

#[cfg(not(unix))]
fn file_identity(_metadata: &std::fs::Metadata) -> FileIdentity {
    unreachable!("identity support is checked before database access")
}

#[cfg(not(unix))]
fn link_count(_metadata: &std::fs::Metadata) -> u64 {
    unreachable!("identity support is checked before database access")
}
