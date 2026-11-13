use std::{
    collections::BTreeSet,
    fs::{self, File, OpenOptions},
    io::{Read, Write},
    path::{Component, Path, PathBuf},
};

use flate2::read::GzDecoder;
use tokio_util::sync::CancellationToken;

use super::{MAX_ARCHIVE_BYTES, MAX_DEPTH, MAX_ENTRIES, invalid};
use crate::plugins::{PluginError, intake::require_active};

pub(super) fn extract(
    archive: &Path,
    destination: &Path,
    prefix: &str,
    path: Option<&str>,
    cancellation: &CancellationToken,
) -> Result<(), PluginError> {
    extract_limited(
        archive,
        destination,
        prefix,
        path,
        cancellation,
        MAX_ARCHIVE_BYTES + 32 * 1_024 * 1_024,
    )
}

pub(super) fn extract_limited(
    archive: &Path,
    destination: &Path,
    prefix: &str,
    path: Option<&str>,
    cancellation: &CancellationToken,
    decompressed_limit: u64,
) -> Result<(), PluginError> {
    let file = File::open(archive).map_err(|source| PluginError::Io {
        action: "read GitHub archive",
        path: archive.to_path_buf(),
        source,
    })?;
    let decoder = GzDecoder::new(file);
    let mut archive = tar::Archive::new(BoundedReader {
        reader: decoder,
        remaining: decompressed_limit,
        cancellation: cancellation.clone(),
    });
    let mut seen = BTreeSet::new();
    let mut bytes = 0_u64;
    let mut count = 0_usize;
    let mut selected = 0_usize;
    for entry in archive
        .entries()
        .map_err(|_| invalid("GitHub source is not a valid gzip tar archive"))?
    {
        require_active(cancellation)?;
        let mut entry =
            entry.map_err(|_| archive_error(cancellation, "GitHub archive entry is malformed"))?;
        count += 1;
        if count > MAX_ENTRIES {
            return Err(invalid("GitHub repository exceeds 16384 archive entries"));
        }
        let entry_path = entry
            .path()
            .map_err(|_| invalid("GitHub archive path is malformed"))?
            .into_owned();
        let path_text = entry_path
            .to_str()
            .ok_or_else(|| invalid("GitHub archive path is not UTF-8"))?;
        relative(path_text.trim_end_matches('/'))?;
        let components = entry_path.components().collect::<Vec<_>>();
        if !components
            .first()
            .and_then(|component| component.as_os_str().to_str())
            .is_some_and(|root| same_root(root, prefix))
        {
            return Err(invalid(
                "GitHub archive does not match the pinned repository commit",
            ));
        }
        if !seen.insert(entry_path.clone()) {
            return Err(invalid("GitHub archive repeats a path"));
        }
        let kind = entry.header().entry_type();
        let size = entry.size();
        bytes = bytes
            .checked_add(size)
            .ok_or_else(|| invalid("GitHub extracted byte count overflowed"))?;
        if bytes > MAX_ARCHIVE_BYTES || size > 32 * 1_024 * 1_024 {
            return Err(invalid(
                "GitHub extracted source exceeds its file or total byte boundary",
            ));
        }
        let relative_path = components.iter().skip(1).collect::<PathBuf>();
        let relative_path = if let Some(subdirectory) = path {
            match relative_path.strip_prefix(subdirectory) {
                Ok(path) => path.to_path_buf(),
                Err(_) => continue,
            }
        } else {
            relative_path
        };
        if !kind.is_file() && !kind.is_dir() {
            return Err(invalid(
                "GitHub archive contains a symlink, hard link, or special file",
            ));
        }
        if kind.is_dir() {
            continue;
        }
        if relative_path.as_os_str().is_empty() {
            return Err(invalid("GitHub source selection must be a directory"));
        }
        write_entry(
            &mut entry,
            destination.join(relative_path),
            size,
            cancellation,
        )?;
        selected += 1;
    }
    verify_trailer(archive.into_inner(), cancellation)?;
    if selected == 0 {
        return Err(invalid("GitHub source directory contains no files"));
    }
    Ok(())
}

// Consume the gzip trailer so tar end markers cannot hide a truncated stream.
fn verify_trailer<R: Read>(
    mut decoder: R,
    cancellation: &CancellationToken,
) -> Result<(), PluginError> {
    let mut remainder = [0_u8; 8192];
    let mut trailing = 0_usize;
    loop {
        require_active(cancellation)?;
        let count = decoder
            .read(&mut remainder)
            .map_err(|_| archive_error(cancellation, "GitHub archive gzip checksum is invalid"))?;
        if count == 0 {
            break;
        }
        trailing += count;
        if trailing > 1_024 * 1_024 || remainder[..count].iter().any(|byte| *byte != 0) {
            return Err(invalid("GitHub archive has unexpected trailing data"));
        }
    }
    Ok(())
}

fn write_entry<R: Read>(
    entry: &mut tar::Entry<'_, R>,
    target: PathBuf,
    size: u64,
    cancellation: &CancellationToken,
) -> Result<(), PluginError> {
    if let Some(parent) = target.parent() {
        fs::create_dir_all(parent).map_err(|source| PluginError::Io {
            action: "create GitHub source directory",
            path: parent.to_path_buf(),
            source,
        })?;
    }
    let mut output = OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(&target)
        .map_err(|source| PluginError::Io {
            action: "create GitHub source file",
            path: target.clone(),
            source,
        })?;
    let copied = std::io::copy(entry, &mut output)
        .map_err(|_| archive_error(cancellation, "GitHub archive file ended unsuccessfully"))?;
    if copied != size {
        return Err(invalid("GitHub archive file ended early"));
    }
    output.flush().map_err(|source| PluginError::Io {
        action: "flush GitHub source file",
        path: target.clone(),
        source,
    })?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        let mode = entry
            .header()
            .mode()
            .map_err(|_| invalid("GitHub archive file mode is malformed"))?;
        fs::set_permissions(
            &target,
            fs::Permissions::from_mode(if mode & 0o111 == 0 { 0o600 } else { 0o700 }),
        )
        .map_err(|source| PluginError::Io {
            action: "set GitHub file permissions",
            path: target,
            source,
        })?;
    }
    Ok(())
}

pub(super) struct BoundedReader<R> {
    pub(super) reader: R,
    pub(super) remaining: u64,
    pub(super) cancellation: CancellationToken,
}

impl<R: Read> Read for BoundedReader<R> {
    fn read(&mut self, output: &mut [u8]) -> std::io::Result<usize> {
        if self.cancellation.is_cancelled() {
            return Err(std::io::Error::other("plugin intake cancelled"));
        }
        if output.is_empty() {
            return Ok(0);
        }
        if self.remaining == 0 {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "decompressed archive exceeds its byte boundary",
            ));
        }
        let limit = usize::try_from(self.remaining)
            .unwrap_or(usize::MAX)
            .min(output.len());
        let read = self.reader.read(&mut output[..limit])?;
        self.remaining -= read as u64;
        Ok(read)
    }
}

fn same_root(observed: &str, expected: &str) -> bool {
    match (observed.rsplit_once('-'), expected.rsplit_once('-')) {
        (Some((observed_repo, observed_commit)), Some((expected_repo, expected_commit))) => {
            observed_repo.eq_ignore_ascii_case(expected_repo) && observed_commit == expected_commit
        }
        _ => false,
    }
}

pub(super) fn relative(value: &str) -> Result<(), PluginError> {
    if value.is_empty()
        || value.contains('\\')
        || value
            .split('/')
            .any(|part| part.is_empty() || part == "." || part == "..")
        || value.chars().any(char::is_control)
        || Path::new(value).components().count() > MAX_DEPTH + 1
        || Path::new(value)
            .components()
            .any(|component| !matches!(component, Component::Normal(_)))
    {
        return Err(invalid(
            "GitHub path must be a bounded relative directory without traversal",
        ));
    }
    Ok(())
}

fn archive_error(cancellation: &CancellationToken, message: &str) -> PluginError {
    if cancellation.is_cancelled() {
        PluginError::Cancelled
    } else {
        invalid(message)
    }
}
