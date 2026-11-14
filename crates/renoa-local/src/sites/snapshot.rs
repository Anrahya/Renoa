//! Snapshot staging, symlink-free tree copying, path resolution, and
//! directory listings for published static sites.

use std::{
    fmt::Write as _,
    fs::{self, DirBuilder, File, OpenOptions},
    io::{self, Read as _},
    path::{Component, Path, PathBuf},
};

#[cfg(unix)]
use std::os::unix::fs::{DirBuilderExt as _, OpenOptionsExt as _};

use rusqlite::{OptionalExtension as _, params};

use super::{MAX_SITE_FILES, MAX_SITE_TOTAL_BYTES, SiteError};
use crate::host::catalog::{self, HostCatalogError};

pub(crate) const SITES_RELATIVE_SUBDIR: &str = "sites";

pub(super) struct DirCleanup(Option<PathBuf>);

impl DirCleanup {
    pub(super) fn armed(path: PathBuf) -> Self {
        Self(Some(path))
    }

    pub(super) fn disarm(&mut self) {
        self.0 = None;
    }
}

impl Drop for DirCleanup {
    fn drop(&mut self) {
        if let Some(path) = self.0.take() {
            let _ = fs::remove_dir_all(path);
        }
    }
}

#[derive(Default)]
pub(super) struct CopyStats {
    pub(super) file_count: usize,
    pub(super) total_bytes: u64,
}

pub(crate) fn validate_site_name(name: &str) -> Result<String, SiteError> {
    let bytes = name.as_bytes();
    if bytes.is_empty() || bytes.len() > 63 {
        return Err(SiteError::InvalidName);
    }
    let is_alnum = |b: u8| b.is_ascii_lowercase() || b.is_ascii_digit();
    if !is_alnum(bytes[0]) || !is_alnum(bytes[bytes.len() - 1]) {
        return Err(SiteError::InvalidName);
    }
    if !bytes.iter().all(|&b| is_alnum(b) || b == b'-') {
        return Err(SiteError::InvalidName);
    }
    Ok(name.to_owned())
}

pub(super) fn resolve_source_folder(
    workspace: &Path,
    folder: &str,
) -> Result<(PathBuf, String), SiteError> {
    let trimmed = folder.trim();
    if trimmed.is_empty() || trimmed.contains('\0') {
        return Err(SiteError::InvalidFolder(
            "provide a folder path inside the workspace".to_owned(),
        ));
    }
    let path = Path::new(trimmed);
    if path.components().any(|c| {
        matches!(
            c,
            Component::Prefix(_) | Component::RootDir | Component::ParentDir
        )
    }) {
        return Err(SiteError::InvalidFolder(
            "folder must stay inside the workspace without `..` or leading `/`".to_owned(),
        ));
    }
    let canonical_ws = fs::canonicalize(workspace)
        .map_err(|_| SiteError::InvalidFolder("workspace directory is unavailable".to_owned()))?;
    let joined = workspace.join(path);
    let meta = fs::symlink_metadata(&joined)
        .map_err(|err| SiteError::InvalidFolder(format!("cannot open `{trimmed}`: {err}")))?;
    if meta.file_type().is_symlink() || !meta.is_dir() {
        return Err(SiteError::InvalidFolder(format!(
            "`{trimmed}` must be a real folder inside the workspace"
        )));
    }
    let resolved = fs::canonicalize(&joined)
        .map_err(|err| SiteError::InvalidFolder(format!("cannot resolve `{trimmed}`: {err}")))?;
    if !resolved.starts_with(&canonical_ws) {
        return Err(SiteError::InvalidFolder(
            "folder resolves outside the workspace".to_owned(),
        ));
    }
    let normalized = resolved
        .strip_prefix(&canonical_ws)
        .ok()
        .and_then(|rel| {
            if rel.as_os_str().is_empty() {
                Some(".".to_owned())
            } else {
                rel.to_str().map(str::to_owned)
            }
        })
        .ok_or_else(|| SiteError::InvalidFolder("folder path is not UTF-8".into()))?;
    Ok((resolved, normalized))
}

pub(crate) fn sites_dir_for_database(database: &Path) -> PathBuf {
    database
        .parent()
        .unwrap_or_else(|| Path::new("."))
        .join(SITES_RELATIVE_SUBDIR)
}

pub(super) fn ensure_sites_dir(database: &Path) -> Result<PathBuf, SiteError> {
    let state_dir = database.parent().unwrap_or_else(|| Path::new("."));
    let state_meta = fs::symlink_metadata(state_dir)?;
    if state_meta.file_type().is_symlink() || !state_meta.is_dir() {
        return Err(SiteError::InvalidFolder(format!(
            "`{}` must be a plain directory, not a symbolic link",
            state_dir.display()
        )));
    }
    let sites = state_dir.join(SITES_RELATIVE_SUBDIR);
    match fs::symlink_metadata(&sites) {
        Ok(meta) if meta.file_type().is_symlink() || !meta.is_dir() => {
            return Err(SiteError::InvalidFolder(format!(
                "`{}` must be a plain directory, not a symbolic link",
                sites.display()
            )));
        }
        Ok(_) => {}
        Err(err) if err.kind() == io::ErrorKind::NotFound => {
            create_private_dir(&sites)?;
        }
        Err(err) => return Err(err.into()),
    }
    let canonical_state = fs::canonicalize(state_dir)?;
    let canonical_sites = fs::canonicalize(&sites)?;
    if canonical_sites != canonical_state.join(SITES_RELATIVE_SUBDIR) {
        return Err(SiteError::InvalidFolder(format!(
            "`{}` must be the plain `sites` directory under state",
            sites.display()
        )));
    }
    Ok(sites)
}

pub(super) fn create_private_dir(path: &Path) -> io::Result<()> {
    let mut builder = DirBuilder::new();
    #[cfg(unix)]
    builder.mode(0o700);
    builder.create(path)
}

pub(super) fn copy_verified_tree(
    src: &Path,
    dst: &Path,
    stats: &mut CopyStats,
) -> Result<(), SiteError> {
    let mut entries = fs::read_dir(src)?
        .collect::<Result<Vec<_>, _>>()?
        .into_iter()
        .map(|entry| entry.path())
        .collect::<Vec<_>>();
    entries.sort();
    for entry in entries {
        let meta = fs::symlink_metadata(&entry)?;
        let kind = meta.file_type();
        let name = entry
            .file_name()
            .ok_or_else(|| SiteError::InvalidFolder("entry has no file name".into()))?;
        let target = dst.join(name);
        if kind.is_symlink() {
            return Err(SiteError::InvalidFolder(format!(
                "`{}` is a symbolic link; static site folders must not contain symbolic links",
                name.to_string_lossy()
            )));
        }
        if kind.is_dir() {
            create_private_dir(&target)?;
            copy_verified_tree(&entry, &target, stats)?;
        } else if kind.is_file() {
            stats.file_count += 1;
            if stats.file_count > MAX_SITE_FILES {
                return Err(SiteError::TooManyFiles {
                    file_count: stats.file_count,
                });
            }
            let room = MAX_SITE_TOTAL_BYTES - stats.total_bytes;
            if meta.len() > room {
                return Err(SiteError::SiteTooLarge {
                    total_bytes: stats.total_bytes.saturating_add(meta.len()),
                });
            }
            let mut input = File::open(&entry)?;
            let mut options = OpenOptions::new();
            options.create_new(true).write(true);
            #[cfg(unix)]
            options.mode(0o400);
            let mut output = options.open(&target)?;
            // A file that grows while it is copied stops one byte past the room.
            let copied = io::copy(&mut input.by_ref().take(room + 1), &mut output)?;
            stats.total_bytes = stats.total_bytes.saturating_add(copied);
            if stats.total_bytes > MAX_SITE_TOTAL_BYTES {
                return Err(SiteError::SiteTooLarge {
                    total_bytes: stats.total_bytes,
                });
            }
        } else {
            return Err(SiteError::InvalidFolder(format!(
                "`{}` is not a regular file or folder",
                name.to_string_lossy()
            )));
        }
    }
    Ok(())
}

/// What a path in a published site names.
#[derive(Debug, PartialEq, Eq)]
pub enum SiteEntry {
    /// A file of the snapshot, to be served as it is.
    File(PathBuf),
    /// The root of a site without an `index.html`: an HTML list of its files.
    Listing(String),
}

/// `relative_path` in site `name`: a file, or a folder's `index.html` when
/// the path is empty or ends in `/`. No segment may be empty, `.`, `..`, or
/// hold a backslash or NUL, and no step may be a symbolic link.
pub(super) fn resolve_entry(
    database: &Path,
    sites_dir: &Path,
    name: &str,
    relative_path: &str,
) -> Result<SiteEntry, SiteError> {
    let name = validate_site_name(name).map_err(|_| SiteError::NotFound)?;
    let db = catalog::open_read_only(database)?;
    let snapshot_id: Option<String> = db
        .query_row(
            "SELECT snapshot_id FROM host_sites WHERE site_name = ?1",
            params![name],
            |row| row.get(0),
        )
        .optional()
        .map_err(HostCatalogError::from)?;
    let snapshot_dir = sites_dir.join(snapshot_id.ok_or(SiteError::NotFound)?);
    let snap_meta = fs::symlink_metadata(&snapshot_dir).map_err(unreachable)?;
    if snap_meta.file_type().is_symlink() || !snap_meta.is_dir() {
        return Err(SiteError::NotFound);
    }
    if relative_path.is_empty() {
        return match regular_file(&snapshot_dir.join("index.html")) {
            Some(index) => Ok(SiteEntry::File(index)),
            None => Ok(SiteEntry::Listing(directory_listing_html(
                &name,
                &snapshot_dir,
            )?)),
        };
    }
    let mut segments = relative_path.split('/').collect::<Vec<_>>();
    if let Some(last) = segments.last_mut()
        && last.is_empty()
    {
        *last = "index.html";
    }
    let mut cursor = snapshot_dir;
    for segment in segments {
        if segment.is_empty() || segment == "." || segment == ".." || segment.contains(['\\', '\0'])
        {
            return Err(SiteError::NotFound);
        }
        cursor.push(segment);
        let meta = fs::symlink_metadata(&cursor).map_err(unreachable)?;
        if meta.file_type().is_symlink() {
            return Err(SiteError::NotFound);
        }
    }
    regular_file(&cursor)
        .map(SiteEntry::File)
        .ok_or(SiteError::NotFound)
}

/// `path` when it is a regular file, not a folder or a symbolic link.
fn regular_file(path: &Path) -> Option<PathBuf> {
    let meta = fs::symlink_metadata(path).ok()?;
    (!meta.file_type().is_symlink() && meta.is_file()).then(|| path.to_path_buf())
}

fn directory_listing_html(site_name: &str, snapshot_dir: &Path) -> Result<String, SiteError> {
    let mut paths = Vec::new();
    collect_relative_paths(snapshot_dir, "", &mut paths)?;
    paths.sort();
    let mut out = String::new();
    let escaped_title = html_escape(site_name);
    let _ = write!(
        out,
        "<!doctype html><html><head><meta charset=\"utf-8\"><title>{escaped_title}</title></head><body><h1>{escaped_title}</h1><ul>"
    );
    for rel in paths {
        let href = rel
            .split('/')
            .map(percent_encode_segment)
            .collect::<Vec<_>>()
            .join("/");
        let label = html_escape(&rel);
        let _ = write!(out, "<li><a href=\"{href}\">{label}</a></li>");
    }
    out.push_str("</ul></body></html>");
    Ok(out)
}

fn collect_relative_paths(dir: &Path, prefix: &str, out: &mut Vec<String>) -> io::Result<()> {
    let mut entries = fs::read_dir(dir)?.collect::<Result<Vec<_>, _>>()?;
    entries.sort_by_key(std::fs::DirEntry::file_name);
    for entry in entries {
        let meta = fs::symlink_metadata(entry.path())?;
        if meta.file_type().is_symlink() {
            continue;
        }
        let name = entry.file_name().to_string_lossy().into_owned();
        let rel = if prefix.is_empty() {
            name
        } else {
            format!("{prefix}/{name}")
        };
        if meta.is_dir() {
            collect_relative_paths(&entry.path(), &rel, out)?;
        } else if meta.is_file() {
            out.push(rel);
        }
    }
    Ok(())
}

fn html_escape(text: &str) -> String {
    let mut escaped = String::with_capacity(text.len());
    for ch in text.chars() {
        match ch {
            '&' => escaped.push_str("&amp;"),
            '<' => escaped.push_str("&lt;"),
            '>' => escaped.push_str("&gt;"),
            '"' => escaped.push_str("&quot;"),
            '\'' => escaped.push_str("&#39;"),
            other => escaped.push(other),
        }
    }
    escaped
}

fn percent_encode_segment(segment: &str) -> String {
    segment
        .bytes()
        .map(|b| {
            if b.is_ascii_alphanumeric() || b"-._~".contains(&b) {
                char::from(b).to_string()
            } else {
                format!("%{b:02X}")
            }
        })
        .collect()
}

pub(super) fn unreachable(error: io::Error) -> SiteError {
    match error.kind() {
        io::ErrorKind::NotFound
        | io::ErrorKind::NotADirectory
        | io::ErrorKind::PermissionDenied => SiteError::NotFound,
        _ => error.into(),
    }
}
