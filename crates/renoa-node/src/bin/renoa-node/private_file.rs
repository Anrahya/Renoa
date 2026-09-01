use std::{
    env,
    io::Write as _,
    path::{Path, PathBuf},
};

use crate::error::ServiceError;

const MAX_CONFIG_BYTES: u64 = 1024 * 1024;
const MAX_SECRET_BYTES: u64 = 16 * 1024;

pub(crate) fn read_config(path: &Path) -> Result<Vec<u8>, ServiceError> {
    read_private(path, MAX_CONFIG_BYTES, "configuration")
}

pub(crate) fn read_secret(path: &Path) -> Result<Vec<u8>, ServiceError> {
    read_private(path, MAX_SECRET_BYTES, "secret")
}

pub(crate) fn require_new_secret_path(path: &Path) -> Result<(), ServiceError> {
    require_absolute(path, "credential output")?;
    let Some(parent) = path.parent() else {
        return Err(ServiceError::Configuration(
            "credential output must have a parent directory".to_owned(),
        ));
    };
    let metadata =
        std::fs::metadata(parent).map_err(|error| ServiceError::file("inspect", parent, error))?;
    if !metadata.is_dir() {
        return Err(ServiceError::Configuration(format!(
            "credential output parent `{}` must be a directory",
            parent.display()
        )));
    }
    match std::fs::symlink_metadata(path) {
        Ok(_) => Err(ServiceError::Configuration(format!(
            "credential output `{}` already exists",
            path.display()
        ))),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(ServiceError::file("inspect", path, error)),
    }
}

pub(crate) fn write_new_secret(path: &Path, contents: &[u8]) -> Result<(), std::io::Error> {
    let mut options = std::fs::OpenOptions::new();
    options.create_new(true).write(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt as _;
        options.mode(0o600);
    }
    let mut file = options.open(path)?;
    let result = file.write_all(contents).and_then(|()| file.sync_all());
    if result.is_err() {
        drop(file);
        let _ = std::fs::remove_file(path);
    }
    result
}

fn read_private(path: &Path, limit: u64, label: &str) -> Result<Vec<u8>, ServiceError> {
    require_absolute(path, label)?;
    let metadata = std::fs::symlink_metadata(path)
        .map_err(|error| ServiceError::file("inspect", path, error))?;
    if !metadata.file_type().is_file() || metadata.file_type().is_symlink() {
        return Err(ServiceError::Configuration(format!(
            "{label} path `{}` must name a regular file",
            path.display()
        )));
    }
    if metadata.len() == 0 || metadata.len() > limit {
        return Err(ServiceError::Configuration(format!(
            "{label} file `{}` must contain 1 through {limit} bytes",
            path.display()
        )));
    }
    require_private(path, &metadata, label)?;
    std::fs::read(path).map_err(|error| ServiceError::file("read", path, error))
}

pub(crate) fn require_absolute(path: &Path, label: &str) -> Result<(), ServiceError> {
    if path.is_absolute() {
        Ok(())
    } else {
        Err(ServiceError::Configuration(format!(
            "{label} path `{}` must be absolute",
            path.display()
        )))
    }
}

#[cfg(unix)]
fn require_private(
    path: &Path,
    metadata: &std::fs::Metadata,
    label: &str,
) -> Result<(), ServiceError> {
    let credentials_directory = env::var_os("CREDENTIALS_DIRECTORY").map(PathBuf::from);
    require_private_in(path, metadata, credentials_directory.as_deref(), label)
}

#[cfg(unix)]
fn require_private_in(
    path: &Path,
    metadata: &std::fs::Metadata,
    credentials_directory: Option<&Path>,
    label: &str,
) -> Result<(), ServiceError> {
    use std::os::unix::fs::{MetadataExt as _, PermissionsExt as _};

    let mode = metadata.permissions().mode() & 0o777;
    if mode.trailing_zeros() >= 6 {
        return Ok(());
    }
    if mode == 0o440 {
        let Some(directory) =
            credentials_directory.filter(|directory| path.parent() == Some(*directory))
        else {
            return Err(private_error(path, label));
        };
        let directory_metadata = std::fs::symlink_metadata(directory)
            .map_err(|error| ServiceError::file("inspect", directory, error))?;
        let directory_mode = directory_metadata.permissions().mode() & 0o777;
        if directory_metadata.file_type().is_dir()
            && !directory_metadata.file_type().is_symlink()
            && matches!(directory_mode, 0o500 | 0o550)
            && directory_metadata.uid() == metadata.uid()
            && directory_metadata.gid() == metadata.gid()
        {
            return Ok(());
        }
    }
    Err(private_error(path, label))
}

#[cfg(not(unix))]
fn require_private(
    _path: &Path,
    _metadata: &std::fs::Metadata,
    _label: &str,
) -> Result<(), ServiceError> {
    Ok(())
}

fn private_error(path: &Path, label: &str) -> ServiceError {
    ServiceError::Configuration(format!(
        "{label} file `{}` must not be accessible by group or other users",
        path.display()
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(unix)]
    #[test]
    fn private_files_reject_public_modes_and_accept_systemd_credentials() {
        use std::os::unix::fs::PermissionsExt as _;

        let root = tempfile::tempdir().expect("temporary directory");
        let path = root.path().join("secret");
        std::fs::write(&path, b"secret").expect("write secret");
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644))
            .expect("set public mode");
        assert!(read_secret(&path).is_err());

        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600))
            .expect("set private mode");
        assert_eq!(read_secret(&path).expect("read private secret"), b"secret");

        let directory = root.path().join("credentials");
        std::fs::create_dir(&directory).expect("create credential directory");
        let mounted = directory.join("node-config");
        std::fs::write(&mounted, b"{}").expect("write mounted credential");
        std::fs::set_permissions(&mounted, std::fs::Permissions::from_mode(0o440))
            .expect("set mounted credential mode");
        std::fs::set_permissions(&directory, std::fs::Permissions::from_mode(0o550))
            .expect("set credential directory mode");
        let metadata = std::fs::symlink_metadata(&mounted).expect("inspect mounted credential");
        require_private_in(&mounted, &metadata, Some(&directory), "configuration")
            .expect("accept systemd credential mount");
        assert!(require_private_in(&mounted, &metadata, None, "configuration").is_err());
    }

    #[cfg(unix)]
    #[test]
    fn new_secret_creation_never_overwrites_and_uses_owner_only_mode() {
        use std::os::unix::fs::PermissionsExt as _;

        let directory = tempfile::tempdir().expect("temporary directory");
        let path = directory.path().join("device.json");
        require_new_secret_path(&path).expect("unused output path");
        write_new_secret(&path, b"first").expect("write new secret");
        assert_eq!(
            std::fs::metadata(&path)
                .expect("secret metadata")
                .permissions()
                .mode()
                & 0o777,
            0o600
        );
        assert!(require_new_secret_path(&path).is_err());
        assert!(write_new_secret(&path, b"second").is_err());
        assert_eq!(std::fs::read(&path).expect("read secret"), b"first");
    }
}
