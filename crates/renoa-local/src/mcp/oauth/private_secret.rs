use std::{
    io::Write as _,
    path::{Path, PathBuf},
};

use crate::mcp::{McpHostError, McpOAuthError, hex_sha256, validate_identity};

const DIRECTORY_MODE: u32 = 0o700;
const FILE_MODE: u32 = 0o600;

#[derive(Clone)]
pub(crate) struct PrivateSecretStore {
    directory: PathBuf,
}

impl PrivateSecretStore {
    pub(crate) fn initialize(directory: PathBuf) -> Result<Self, McpHostError> {
        if !directory.is_absolute() {
            return Err(McpOAuthError::Invalid(
                "private Host credential directory must be absolute".to_owned(),
            )
            .into());
        }
        std::fs::create_dir_all(&directory)?;
        let metadata = std::fs::symlink_metadata(&directory)?;
        if !metadata.file_type().is_dir() || metadata.file_type().is_symlink() {
            return Err(McpOAuthError::Invalid(
                "private Host credential path must be a real directory".to_owned(),
            )
            .into());
        }
        restrict_directory(&directory)?;
        Ok(Self {
            directory: std::fs::canonicalize(directory)?,
        })
    }

    pub(crate) async fn lookup(
        &self,
        credential_id: &str,
        limit: usize,
    ) -> Result<Option<Vec<u8>>, McpHostError> {
        validate_identity("credential", credential_id)?;
        let path = self.path(credential_id);
        tokio::task::spawn_blocking(move || read_secret(&path, limit)).await?
    }

    pub(crate) async fn store(
        &self,
        credential_id: &str,
        bytes: Vec<u8>,
    ) -> Result<(), McpHostError> {
        validate_identity("credential", credential_id)?;
        let directory = self.directory.clone();
        let path = self.path(credential_id);
        tokio::task::spawn_blocking(move || write_secret(&directory, &path, bytes)).await?
    }

    pub(crate) async fn delete(&self, credential_id: &str) -> Result<(), McpHostError> {
        validate_identity("credential", credential_id)?;
        let path = self.path(credential_id);
        tokio::task::spawn_blocking(move || match std::fs::remove_file(path) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(McpHostError::from(error)),
        })
        .await?
    }

    fn path(&self, credential_id: &str) -> PathBuf {
        let mut identity = b"renoa private oauth secret v1\0".to_vec();
        identity.extend_from_slice(credential_id.as_bytes());
        self.directory
            .join(format!("{}.json", hex_sha256(&identity)))
    }
}

fn read_secret(path: &Path, limit: usize) -> Result<Option<Vec<u8>>, McpHostError> {
    let metadata = match std::fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error.into()),
    };
    if !metadata.file_type().is_file()
        || metadata.file_type().is_symlink()
        || metadata.len() == 0
        || metadata.len() > limit as u64
    {
        return Err(McpOAuthError::Invalid(
            "private Host credential is malformed or exceeds its boundary".to_owned(),
        )
        .into());
    }
    require_private_file(&metadata)?;
    let bytes = std::fs::read(path)?;
    if bytes.is_empty() || bytes.len() > limit {
        return Err(McpOAuthError::Invalid(
            "private Host credential is malformed or exceeds its boundary".to_owned(),
        )
        .into());
    }
    Ok(Some(bytes))
}

fn write_secret(directory: &Path, path: &Path, mut bytes: Vec<u8>) -> Result<(), McpHostError> {
    if bytes.is_empty() {
        return Err(
            McpOAuthError::Invalid("private Host credential cannot be empty".to_owned()).into(),
        );
    }
    if let Ok(metadata) = std::fs::symlink_metadata(path)
        && (!metadata.file_type().is_file() || metadata.file_type().is_symlink())
    {
        bytes.fill(0);
        return Err(McpOAuthError::Invalid(
            "private Host credential destination is not a regular file".to_owned(),
        )
        .into());
    }
    let temporary = directory.join(format!(".oauth-secret-{}.tmp", uuid::Uuid::new_v4()));
    let result = (|| {
        let mut options = std::fs::OpenOptions::new();
        options.create_new(true).write(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt as _;
            options.mode(FILE_MODE);
        }
        let mut file = options.open(&temporary)?;
        file.write_all(&bytes)?;
        file.sync_all()?;
        drop(file);
        std::fs::rename(&temporary, path)?;
        std::fs::File::open(directory)?.sync_all()
    })();
    bytes.fill(0);
    if result.is_err() {
        let _ignored = std::fs::remove_file(&temporary);
    }
    result.map_err(McpHostError::from)
}

#[cfg(unix)]
fn restrict_directory(path: &Path) -> Result<(), McpHostError> {
    use std::os::unix::fs::PermissionsExt as _;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(DIRECTORY_MODE))?;
    Ok(())
}

#[cfg(not(unix))]
fn restrict_directory(_path: &Path) -> Result<(), McpHostError> {
    Ok(())
}

#[cfg(unix)]
fn require_private_file(metadata: &std::fs::Metadata) -> Result<(), McpHostError> {
    use std::os::unix::fs::PermissionsExt as _;
    if metadata.permissions().mode().trailing_zeros() >= 6 {
        Ok(())
    } else {
        Err(McpOAuthError::Invalid(
            "private Host credential is accessible by group or other users".to_owned(),
        )
        .into())
    }
}

#[cfg(not(unix))]
fn require_private_file(_metadata: &std::fs::Metadata) -> Result<(), McpHostError> {
    Ok(())
}

#[cfg(test)]
mod tests {
    use tempfile::tempdir;

    use super::PrivateSecretStore;

    #[tokio::test]
    async fn private_store_round_trips_one_owner_only_atomic_file() {
        let root = tempdir().expect("temporary private secret root");
        let directory = root.path().join("oauth-secrets");
        let store = PrivateSecretStore::initialize(directory.clone())
            .expect("initialize private secret store");
        store
            .store("oauth.example", b"secret-one".to_vec())
            .await
            .expect("store first secret");
        store
            .store("oauth.example", b"secret-two".to_vec())
            .await
            .expect("replace secret atomically");
        assert_eq!(
            store
                .lookup("oauth.example", 64)
                .await
                .expect("load private secret")
                .as_deref(),
            Some(b"secret-two".as_slice())
        );
        let entries = std::fs::read_dir(&directory)
            .expect("list private secret directory")
            .collect::<Result<Vec<_>, _>>()
            .expect("read private secret entries");
        assert_eq!(entries.len(), 1);

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;

            assert_eq!(
                std::fs::metadata(&directory)
                    .expect("private directory metadata")
                    .permissions()
                    .mode()
                    & 0o777,
                0o700
            );
            assert_eq!(
                entries[0]
                    .metadata()
                    .expect("private file metadata")
                    .permissions()
                    .mode()
                    & 0o777,
                0o600
            );
        }
    }
}
