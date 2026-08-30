use std::{
    collections::BTreeSet,
    fs::{self, File, OpenOptions},
    io::{Read as _, Write as _},
    path::{Component, Path, PathBuf},
};

use renoa_registry_protocol::Sha256Digest;
use sha2::{Digest as _, Sha256};

use super::SharedRegistryError;
use crate::{
    package_tree::{self, CapturedFile, TreeLimits},
    plugins::{PluginError, inspect, store::PluginStore},
};

pub(super) struct PackageArchive {
    path: tempfile::TempPath,
    digest: Sha256Digest,
    bytes: u64,
}

impl PackageArchive {
    pub(super) fn build(
        store: &PluginStore,
        package_digest: &str,
        transfer: &Path,
    ) -> Result<Self, SharedRegistryError> {
        let root = store.package_root(package_digest)?;
        let tree = package_tree::capture(&root, inspect::digest_domain(), inspect::tree_limits())
            .map_err(PluginError::from_tree)?;
        if tree.digest != package_digest || !tree.skipped_entries.is_empty() {
            return Err(SharedRegistryError::Conflict(format!(
                "installed package {package_digest} changed before publication"
            )));
        }
        let temporary = tempfile::Builder::new()
            .prefix("package-")
            .suffix(".tar")
            .tempfile_in(transfer)?;
        write_archive(temporary.path(), &tree.files)?;
        let bytes = temporary.as_file().metadata()?.len();
        if bytes == 0 || bytes > renoa_registry_protocol::MAX_PACKAGE_ARCHIVE_BYTES {
            return Err(SharedRegistryError::Archive(format!(
                "package archive size {bytes} is outside the supported range"
            )));
        }
        let digest = file_digest(temporary.path())?;
        Ok(Self {
            path: temporary.into_temp_path(),
            digest,
            bytes,
        })
    }

    pub(super) fn path(&self) -> &Path {
        &self.path
    }

    pub(super) const fn digest(&self) -> &Sha256Digest {
        &self.digest
    }

    pub(super) const fn bytes(&self) -> u64 {
        self.bytes
    }
}

pub(super) fn install_archive(
    store: &PluginStore,
    archive: &Path,
    expected_package: &Sha256Digest,
    transfer: &Path,
) -> Result<(), SharedRegistryError> {
    let staging = tempfile::Builder::new()
        .prefix("unpack-")
        .tempdir_in(transfer)?;
    extract_archive(archive, staging.path(), inspect::tree_limits())?;
    store.install(staging.path(), expected_package.as_str())?;
    Ok(())
}

fn write_archive(path: &Path, files: &[CapturedFile]) -> Result<(), SharedRegistryError> {
    let file = OpenOptions::new().write(true).truncate(true).open(path)?;
    let mut builder = tar::Builder::new(file);
    builder.mode(tar::HeaderMode::Deterministic);
    for captured in files {
        let mut header = tar::Header::new_gnu();
        header.set_entry_type(tar::EntryType::Regular);
        header.set_size(u64::try_from(captured.bytes.len()).map_err(|_| {
            SharedRegistryError::Archive("package file size overflowed".to_owned())
        })?);
        header.set_mode(if captured.executable { 0o755 } else { 0o644 });
        header.set_uid(0);
        header.set_gid(0);
        header.set_mtime(0);
        header.set_cksum();
        builder.append_data(&mut header, &captured.relative, captured.bytes.as_slice())?;
    }
    let mut file = builder.into_inner()?;
    file.flush()?;
    file.sync_all()?;
    Ok(())
}

fn extract_archive(
    archive: &Path,
    destination: &Path,
    limits: TreeLimits,
) -> Result<(), SharedRegistryError> {
    let mut archive = tar::Archive::new(File::open(archive)?);
    let mut seen = BTreeSet::new();
    let mut total = 0_u64;
    for entry in archive.entries()? {
        let mut entry = entry?;
        if !entry.header().entry_type().is_file() {
            return Err(SharedRegistryError::Archive(
                "package archive contains a non-file entry".to_owned(),
            ));
        }
        if seen.len() >= limits.max_files {
            return Err(SharedRegistryError::Archive(format!(
                "package archive exceeds {} files",
                limits.max_files
            )));
        }
        let relative = validated_path(&entry.path()?, limits.max_depth)?;
        if !seen.insert(relative.clone()) {
            return Err(SharedRegistryError::Archive(format!(
                "package archive repeats `{}`",
                relative.display()
            )));
        }
        let size = entry.header().size()?;
        if size > limits.max_file_bytes {
            return Err(SharedRegistryError::Archive(format!(
                "package archive file `{}` exceeds {} bytes",
                relative.display(),
                limits.max_file_bytes
            )));
        }
        total = total.checked_add(size).ok_or_else(|| {
            SharedRegistryError::Archive("package archive byte count overflowed".to_owned())
        })?;
        if total > limits.max_total_bytes {
            return Err(SharedRegistryError::Archive(format!(
                "package archive exceeds {} extracted bytes",
                limits.max_total_bytes
            )));
        }
        let target = destination.join(&relative);
        if let Some(parent) = target.parent() {
            fs::create_dir_all(parent)?;
        }
        let mut output = OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(&target)?;
        let copied = std::io::copy(&mut entry, &mut output)?;
        if copied != size {
            return Err(SharedRegistryError::Archive(format!(
                "package archive file `{}` ended early",
                relative.display()
            )));
        }
        output.sync_all()?;
        executable_permissions(&target, entry.header().mode()? & 0o111 != 0)?;
    }
    if seen.is_empty() {
        return Err(SharedRegistryError::Archive(
            "package archive contains no files".to_owned(),
        ));
    }
    Ok(())
}

fn validated_path(path: &Path, max_depth: usize) -> Result<PathBuf, SharedRegistryError> {
    let components = path.components().collect::<Vec<_>>();
    if components.is_empty() || components.len() > max_depth + 1 {
        return Err(SharedRegistryError::Archive(
            "package archive path depth is invalid".to_owned(),
        ));
    }
    let mut validated = PathBuf::new();
    for component in components {
        let Component::Normal(value) = component else {
            return Err(SharedRegistryError::Archive(
                "package archive path escapes its package root".to_owned(),
            ));
        };
        if value.to_str().is_none() {
            return Err(SharedRegistryError::Archive(
                "package archive path is not UTF-8".to_owned(),
            ));
        }
        validated.push(value);
    }
    Ok(validated)
}

fn file_digest(path: &Path) -> Result<Sha256Digest, SharedRegistryError> {
    let mut file = File::open(path)?;
    let mut hasher = Sha256::new();
    let mut buffer = [0_u8; 8 * 1_024];
    loop {
        let read = file.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    Sha256Digest::parse(hex(&hasher.finalize()))
        .map_err(|error| SharedRegistryError::Protocol(error.to_string()))
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
fn executable_permissions(path: &Path, executable: bool) -> Result<(), SharedRegistryError> {
    use std::os::unix::fs::PermissionsExt as _;
    fs::set_permissions(
        path,
        fs::Permissions::from_mode(if executable { 0o700 } else { 0o600 }),
    )?;
    Ok(())
}

#[cfg(not(unix))]
fn executable_permissions(_path: &Path, _executable: bool) -> Result<(), SharedRegistryError> {
    Ok(())
}
