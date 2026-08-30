use std::{
    collections::BTreeSet,
    fs::{self, File},
    io::Read as _,
    path::{Path, PathBuf},
};

use renoa_registry_protocol::Sha256Digest;
use sha2::{Digest as _, Sha256};

use crate::store::RegistryError;

pub(crate) fn acquire_lock(path: &Path) -> Result<File, RegistryError> {
    let file = fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(path)?;
    writable_file(path)?;
    file.try_lock().map_err(|error| {
        RegistryError::InvalidState(format!(
            "registry state directory is already in use: {error}"
        ))
    })?;
    file.sync_all()?;
    Ok(file)
}

pub(crate) fn initialize_directory(path: &Path) -> Result<PathBuf, RegistryError> {
    fs::create_dir_all(path)?;
    let path = fs::canonicalize(path)?;
    if !fs::symlink_metadata(&path)?.file_type().is_dir() {
        return Err(RegistryError::InvalidState(format!(
            "registry path `{}` is not a real directory",
            path.display()
        )));
    }
    owner_only_directory(&path)?;
    Ok(path)
}

pub(crate) fn publish(
    staging: &Path,
    blobs: &Path,
    expected_digest: &Sha256Digest,
    expected_bytes: u64,
) -> Result<PathBuf, RegistryError> {
    verify_file(staging, expected_digest, expected_bytes)?;
    let target = blobs.join(expected_digest.as_str());
    if target.try_exists()? {
        verify_file(&target, expected_digest, expected_bytes)?;
        fs::remove_file(staging)?;
        return Ok(target);
    }
    match fs::rename(staging, &target) {
        Ok(()) => {}
        Err(error)
            if matches!(
                error.kind(),
                std::io::ErrorKind::AlreadyExists | std::io::ErrorKind::PermissionDenied
            ) && target.try_exists()? =>
        {
            verify_file(&target, expected_digest, expected_bytes)?;
            fs::remove_file(staging)?;
            return Ok(target);
        }
        Err(error) => return Err(error.into()),
    }
    readonly_file(&target)?;
    File::open(&target)?.sync_all()?;
    File::open(blobs)?.sync_all()?;
    Ok(target)
}

pub(crate) fn verify_file(
    path: &Path,
    expected_digest: &Sha256Digest,
    expected_bytes: u64,
) -> Result<(), RegistryError> {
    let mut file = File::open(path)?;
    let metadata = file.metadata()?;
    if !metadata.is_file() || metadata.len() != expected_bytes {
        return Err(RegistryError::InvalidState(
            "package archive size differs from its durable record".to_owned(),
        ));
    }
    let mut hasher = Sha256::new();
    let mut buffer = [0_u8; 8 * 1_024];
    loop {
        let read = file.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    let observed = hex(&hasher.finalize());
    if observed != expected_digest.as_str() {
        return Err(RegistryError::InvalidState(
            "package archive content differs from its durable record".to_owned(),
        ));
    }
    Ok(())
}

pub(crate) fn remove_staging(path: &Path) -> Result<(), RegistryError> {
    if path.try_exists()? {
        writable_file(path)?;
        fs::remove_file(path)?;
    }
    Ok(())
}

pub(crate) fn clean_staging(directory: &Path) -> Result<(), RegistryError> {
    let mut changed = false;
    for entry in fs::read_dir(directory)? {
        let entry = entry?;
        let name = entry.file_name();
        let Some(name) = name.to_str() else {
            return Err(RegistryError::InvalidState(
                "registry staging contains a non-UTF-8 entry".to_owned(),
            ));
        };
        let Some(id) = name
            .strip_prefix("upload-")
            .and_then(|value| value.strip_suffix(".tar"))
        else {
            return Err(RegistryError::InvalidState(format!(
                "registry staging contains unexpected entry `{name}`"
            )));
        };
        if uuid::Uuid::parse_str(id).is_err() || !entry.file_type()?.is_file() {
            return Err(RegistryError::InvalidState(format!(
                "registry staging entry `{name}` is invalid"
            )));
        }
        remove_staging(&entry.path())?;
        changed = true;
    }
    sync_directory_if_changed(directory, changed)
}

pub(crate) fn clean_unreferenced(
    directory: &Path,
    referenced: &BTreeSet<String>,
) -> Result<(), RegistryError> {
    let mut changed = false;
    for entry in fs::read_dir(directory)? {
        let entry = entry?;
        let name = entry.file_name();
        let Some(name) = name.to_str() else {
            return Err(RegistryError::InvalidState(
                "registry blob store contains a non-UTF-8 entry".to_owned(),
            ));
        };
        if Sha256Digest::parse(name.to_owned()).is_err() || !entry.file_type()?.is_file() {
            return Err(RegistryError::InvalidState(format!(
                "registry blob store contains unexpected entry `{name}`"
            )));
        }
        if !referenced.contains(name) {
            writable_file(&entry.path())?;
            fs::remove_file(entry.path())?;
            changed = true;
        }
    }
    sync_directory_if_changed(directory, changed)
}

fn sync_directory_if_changed(directory: &Path, changed: bool) -> Result<(), RegistryError> {
    if changed {
        File::open(directory)?.sync_all()?;
    }
    Ok(())
}

fn hex(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut output = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        output.push(char::from(HEX[(byte >> 4) as usize]));
        output.push(char::from(HEX[(byte & 0x0f) as usize]));
    }
    output
}

#[cfg(unix)]
fn owner_only_directory(path: &Path) -> Result<(), RegistryError> {
    use std::os::unix::fs::PermissionsExt as _;
    fs::set_permissions(path, fs::Permissions::from_mode(0o700))?;
    Ok(())
}

#[cfg(not(unix))]
fn owner_only_directory(_path: &Path) -> Result<(), RegistryError> {
    Ok(())
}

#[cfg(unix)]
fn readonly_file(path: &Path) -> Result<(), RegistryError> {
    use std::os::unix::fs::PermissionsExt as _;
    fs::set_permissions(path, fs::Permissions::from_mode(0o400))?;
    Ok(())
}

#[cfg(not(unix))]
fn readonly_file(path: &Path) -> Result<(), RegistryError> {
    let mut permissions = fs::metadata(path)?.permissions();
    permissions.set_readonly(true);
    fs::set_permissions(path, permissions)?;
    Ok(())
}

#[cfg(unix)]
fn writable_file(path: &Path) -> Result<(), RegistryError> {
    use std::os::unix::fs::PermissionsExt as _;
    fs::set_permissions(path, fs::Permissions::from_mode(0o600))?;
    Ok(())
}

#[cfg(not(unix))]
fn writable_file(path: &Path) -> Result<(), RegistryError> {
    let mut permissions = fs::metadata(path)?.permissions();
    permissions.set_readonly(false);
    fs::set_permissions(path, permissions)?;
    Ok(())
}
