use std::path::Path;

#[cfg(unix)]
use std::{
    ffi::OsString,
    fs::{File, OpenOptions},
    path::PathBuf,
};

use rusqlite::Connection;

use crate::{KernelError, StoreError, StoreErrorKind, schema::open_connection};

pub(crate) struct DatabaseLease {
    #[cfg(unix)]
    path: PathBuf,
    #[cfg(unix)]
    database_file: File,
    #[cfg(unix)]
    lock_path: PathBuf,
    #[cfg(unix)]
    owner_lock: File,
}

impl DatabaseLease {
    #[cfg(unix)]
    pub(crate) fn acquire(path: &Path) -> Result<Self, KernelError> {
        let database_file = OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(path)
            .map_err(|error| KernelError::Store(StoreError::io("open", path, error)))?;
        let path = path
            .canonicalize()
            .map_err(|error| KernelError::Store(StoreError::io("resolve", path, error)))?;
        reject_database_links(&database_file, &path)?;

        let lock_path = lock_path(&path);
        let owner_lock = OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(&lock_path)
            .map_err(|error| KernelError::Store(StoreError::io("open", &lock_path, error)))?;
        owner_lock.try_lock().map_err(|error| match error {
            std::fs::TryLockError::WouldBlock => KernelError::AlreadyRunning { path: path.clone() },
            std::fs::TryLockError::Error(error) => {
                KernelError::Store(StoreError::io("lock", &lock_path, error))
            }
        })?;

        let lease = Self {
            path,
            database_file,
            lock_path,
            owner_lock,
        };
        lease.validate_identity()?;
        Ok(lease)
    }

    #[cfg(not(unix))]
    pub(crate) fn acquire(_path: &Path) -> Result<Self, KernelError> {
        Err(unsupported_platform())
    }

    #[cfg(unix)]
    pub(crate) fn connection(&self) -> Result<Connection, KernelError> {
        self.validate_identity()?;
        let connection = open_connection(&self.path)?;
        self.validate_identity()?;
        Ok(connection)
    }

    #[cfg(not(unix))]
    pub(crate) fn connection(&self) -> Result<Connection, KernelError> {
        Err(unsupported_platform())
    }

    #[cfg(unix)]
    fn validate_identity(&self) -> Result<(), KernelError> {
        validate_file_identity(&self.database_file, &self.path, true)?;
        validate_file_identity(&self.owner_lock, &self.lock_path, false)
    }
}

#[cfg(unix)]
fn lock_path(database: &Path) -> PathBuf {
    let mut value: OsString = database.as_os_str().to_owned();
    value.push(".lock");
    PathBuf::from(value)
}

#[cfg(unix)]
fn reject_database_links(database_file: &File, path: &Path) -> Result<(), KernelError> {
    let metadata = database_file
        .metadata()
        .map_err(|error| KernelError::Store(StoreError::io("inspect", path, error)))?;
    if link_count(&metadata) == 1 {
        Ok(())
    } else {
        Err(KernelError::UnsupportedDatabaseAlias {
            path: path.to_owned(),
        })
    }
}

#[cfg(unix)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct FileIdentity {
    device: u64,
    inode: u64,
}

#[cfg(unix)]
fn validate_file_identity(
    open_file: &File,
    path: &Path,
    reject_links: bool,
) -> Result<(), KernelError> {
    let open_metadata = open_file
        .metadata()
        .map_err(|error| KernelError::Store(StoreError::io("inspect open", path, error)))?;
    let path_metadata = path
        .metadata()
        .map_err(|error| KernelError::Store(StoreError::io("inspect", path, error)))?;
    if reject_links && link_count(&open_metadata) != 1 {
        return Err(KernelError::UnsupportedDatabaseAlias {
            path: path.to_owned(),
        });
    }
    if file_identity(&open_metadata) != file_identity(&path_metadata) {
        return Err(KernelError::Store(StoreError::message(
            StoreErrorKind::Identity,
            format!("{} changed while the kernel owned it", path.display()),
        )));
    }
    Ok(())
}

#[cfg(unix)]
fn file_identity(metadata: &std::fs::Metadata) -> FileIdentity {
    use std::os::unix::fs::MetadataExt;

    FileIdentity {
        device: metadata.dev(),
        inode: metadata.ino(),
    }
}

#[cfg(unix)]
fn link_count(metadata: &std::fs::Metadata) -> u64 {
    use std::os::unix::fs::MetadataExt;

    metadata.nlink()
}

#[cfg(not(unix))]
fn unsupported_platform() -> KernelError {
    KernelError::Store(StoreError::message(
        StoreErrorKind::UnsupportedPlatform,
        "the durable kernel currently requires Unix file identity support",
    ))
}
