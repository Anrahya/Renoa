mod publish;

use std::{
    fs::{self, File},
    io::Read as _,
    path::{Component, Path, PathBuf},
};

use sha2::{Digest as _, Sha256};
use thiserror::Error;

pub(crate) use publish::{initialize_store, publish};

#[derive(Clone, Copy)]
pub(crate) struct TreeLimits {
    pub(crate) max_files: usize,
    pub(crate) max_depth: usize,
    pub(crate) max_file_bytes: u64,
    pub(crate) max_total_bytes: u64,
    pub(crate) ignored_root_entries: &'static [&'static str],
    pub(crate) unsupported_entry_policy: UnsupportedEntryPolicy,
}

#[derive(Clone, Copy)]
pub(crate) enum UnsupportedEntryPolicy {
    Reject,
    Skip,
}

#[derive(Debug)]
pub(crate) struct CapturedTree {
    pub(crate) digest: String,
    pub(crate) files: Vec<CapturedFile>,
    pub(crate) directories: Vec<String>,
    pub(crate) skipped_entries: Vec<String>,
}

#[derive(Clone, Debug)]
pub(crate) struct CapturedFile {
    pub(crate) relative: String,
    pub(crate) bytes: Vec<u8>,
    pub(crate) executable: bool,
}

#[derive(Debug, Error)]
pub(crate) enum TreeError {
    #[error("invalid package tree: {0}")]
    Invalid(String),
    #[error("package tree conflicts with durable state: {0}")]
    Conflict(String),
    #[error("package tree failed while {action} `{path}`: {source}")]
    Io {
        action: &'static str,
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
}

impl TreeError {
    fn io(action: &'static str, path: impl Into<PathBuf>, source: std::io::Error) -> Self {
        Self::Io {
            action,
            path: path.into(),
            source,
        }
    }
}

pub(crate) fn capture(
    root: &Path,
    digest_domain: &[u8],
    limits: TreeLimits,
) -> Result<CapturedTree, TreeError> {
    validate_root(root)?;
    let mut pending = vec![(root.to_path_buf(), 0_usize)];
    let mut files = Vec::new();
    let mut directories = Vec::new();
    let mut skipped_entries = Vec::new();
    let mut total_bytes = 0_u64;
    while let Some((directory, depth)) = pending.pop() {
        if depth > limits.max_depth {
            return Err(TreeError::Invalid(format!(
                "package `{}` exceeds directory depth {}",
                root.display(),
                limits.max_depth
            )));
        }
        let mut entries = read_dir_sorted(&directory)?;
        entries.reverse();
        for entry in entries {
            let path = entry.path();
            if directory == root
                && entry
                    .file_name()
                    .to_str()
                    .is_some_and(|name| limits.ignored_root_entries.contains(&name))
            {
                continue;
            }
            let file_type = entry
                .file_type()
                .map_err(|error| TreeError::io("inspect package entry", &path, error))?;
            if file_type.is_symlink() {
                let relative = relative_path(root, &path)?;
                match limits.unsupported_entry_policy {
                    UnsupportedEntryPolicy::Reject => {
                        return Err(TreeError::Invalid(format!(
                            "package `{}` contains symlink `{relative}`",
                            root.display()
                        )));
                    }
                    UnsupportedEntryPolicy::Skip => skipped_entries.push(relative),
                }
                continue;
            }
            if file_type.is_dir() {
                directories.push(relative_path(root, &path)?);
                pending.push((path, depth + 1));
                continue;
            }
            if !file_type.is_file() {
                let relative = relative_path(root, &path)?;
                match limits.unsupported_entry_policy {
                    UnsupportedEntryPolicy::Reject => {
                        return Err(TreeError::Invalid(format!(
                            "package `{}` contains non-file entry `{relative}`",
                            root.display()
                        )));
                    }
                    UnsupportedEntryPolicy::Skip => skipped_entries.push(relative),
                }
                continue;
            }
            if files.len() >= limits.max_files {
                return Err(TreeError::Invalid(format!(
                    "package `{}` exceeds {} files",
                    root.display(),
                    limits.max_files
                )));
            }
            let (bytes, metadata) = read_bounded_file(&path, limits.max_file_bytes)?;
            let length = u64::try_from(bytes.len())
                .map_err(|error| TreeError::Invalid(format!("file size is invalid: {error}")))?;
            total_bytes = total_bytes
                .checked_add(length)
                .ok_or_else(|| TreeError::Invalid("package byte count overflowed".to_owned()))?;
            if total_bytes > limits.max_total_bytes {
                return Err(TreeError::Invalid(format!(
                    "package `{}` exceeds {} bytes",
                    root.display(),
                    limits.max_total_bytes
                )));
            }
            files.push(CapturedFile {
                relative: relative_path(root, &path)?,
                bytes,
                executable: executable(&metadata),
            });
        }
    }
    files.sort_by(|left, right| left.relative.cmp(&right.relative));
    directories.sort();
    Ok(CapturedTree {
        digest: digest(digest_domain, &files),
        files,
        directories,
        skipped_entries,
    })
}

fn validate_root(root: &Path) -> Result<(), TreeError> {
    let metadata = fs::symlink_metadata(root)
        .map_err(|error| TreeError::io("inspect package directory", root, error))?;
    if !metadata.file_type().is_dir() {
        return Err(TreeError::Invalid(format!(
            "package path `{}` is not a real directory",
            root.display()
        )));
    }
    Ok(())
}

pub(crate) fn verify(
    root: &Path,
    expected_digest: &str,
    digest_domain: &[u8],
    limits: TreeLimits,
) -> Result<CapturedTree, TreeError> {
    let captured = capture(root, digest_domain, limits)?;
    if captured.digest == expected_digest {
        Ok(captured)
    } else {
        Err(TreeError::Conflict(format!(
            "installed package `{expected_digest}` no longer matches its content digest"
        )))
    }
}

fn read_bounded_file(path: &Path, limit: u64) -> Result<(Vec<u8>, fs::Metadata), TreeError> {
    let file = File::open(path).map_err(|error| TreeError::io("open package file", path, error))?;
    let metadata = file
        .metadata()
        .map_err(|error| TreeError::io("inspect opened package file", path, error))?;
    if !metadata.is_file() {
        return Err(TreeError::Invalid(format!(
            "package entry `{}` did not open as a regular file",
            path.display()
        )));
    }
    let mut bytes = Vec::new();
    file.take(limit.saturating_add(1))
        .read_to_end(&mut bytes)
        .map_err(|error| TreeError::io("read package file", path, error))?;
    if u64::try_from(bytes.len()).unwrap_or(u64::MAX) > limit {
        return Err(TreeError::Invalid(format!(
            "package file `{}` exceeds {limit} bytes",
            path.display()
        )));
    }
    Ok((bytes, metadata))
}

fn digest(domain: &[u8], files: &[CapturedFile]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(domain);
    for file in files {
        hasher.update((file.relative.len() as u64).to_be_bytes());
        hasher.update(file.relative.as_bytes());
        hasher.update([u8::from(file.executable)]);
        hasher.update((file.bytes.len() as u64).to_be_bytes());
        hasher.update(&file.bytes);
    }
    hex(&hasher.finalize())
}

fn relative_path(root: &Path, path: &Path) -> Result<String, TreeError> {
    let relative = path.strip_prefix(root).map_err(|_| {
        TreeError::Invalid(format!(
            "package path `{}` escaped `{}`",
            path.display(),
            root.display()
        ))
    })?;
    relative
        .components()
        .map(|component| match component {
            Component::Normal(value) => value.to_str().map(str::to_owned).ok_or_else(|| {
                TreeError::Invalid(format!("package path `{}` is not UTF-8", path.display()))
            }),
            _ => Err(TreeError::Invalid(format!(
                "package path `{}` is not contained",
                path.display()
            ))),
        })
        .collect::<Result<Vec<_>, _>>()
        .map(|parts| parts.join("/"))
}

fn read_dir_sorted(path: &Path) -> Result<Vec<fs::DirEntry>, TreeError> {
    let mut entries = fs::read_dir(path)
        .map_err(|error| TreeError::io("read package directory", path, error))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| TreeError::io("read package directory entry", path, error))?;
    entries.sort_by_key(fs::DirEntry::file_name);
    Ok(entries)
}

fn hex(bytes: &[u8]) -> String {
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    let mut encoded = String::with_capacity(bytes.len() * 2);
    for &byte in bytes {
        encoded.push(char::from(DIGITS[usize::from(byte >> 4)]));
        encoded.push(char::from(DIGITS[usize::from(byte & 0x0f)]));
    }
    encoded
}

#[cfg(unix)]
fn executable(metadata: &fs::Metadata) -> bool {
    use std::os::unix::fs::PermissionsExt as _;
    metadata.permissions().mode() & 0o111 != 0
}

#[cfg(not(unix))]
fn executable(_metadata: &fs::Metadata) -> bool {
    false
}
