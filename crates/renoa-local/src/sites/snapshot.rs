//! Snapshot staging, descriptor-relative tree copying, path resolution, and
//! directory listings for published static sites.

use std::{
    ffi::OsStr,
    fmt::Write as _,
    fs::{self, DirBuilder, File, OpenOptions},
    io::{self, Read as _},
    os::{
        fd::AsFd as _,
        unix::{
            ffi::OsStrExt as _,
            fs::{DirBuilderExt as _, OpenOptionsExt as _},
        },
    },
    path::{Component, Path, PathBuf},
};

use nix::{
    dir::Dir,
    errno::Errno,
    fcntl::{OFlag, openat},
    sys::stat::{Mode, SFlag, fstat},
};
use rusqlite::{OptionalExtension as _, params};

use super::{MAX_SITE_ENTRIES, MAX_SITE_TOTAL_BYTES, SiteError};
use crate::host::catalog::{self, HostCatalogError};

pub(crate) const SITES_RELATIVE_SUBDIR: &str = "sites";

pub(crate) struct DirCleanup(Option<PathBuf>);

impl DirCleanup {
    pub(crate) fn armed(path: PathBuf) -> Self {
        Self(Some(path))
    }

    pub(crate) fn disarm(&mut self) {
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
    /// Files and folders seen, which [`MAX_SITE_ENTRIES`] bounds.
    entries: usize,
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

pub(crate) fn resolve_source_folder(
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
    ensure_state_subdir(database, SITES_RELATIVE_SUBDIR)
}

pub(crate) fn ensure_state_subdir(database: &Path, subdir: &str) -> Result<PathBuf, SiteError> {
    let state_dir = database.parent().unwrap_or_else(|| Path::new("."));
    let state_meta = fs::symlink_metadata(state_dir)?;
    if state_meta.file_type().is_symlink() || !state_meta.is_dir() {
        return Err(SiteError::InvalidFolder(format!(
            "`{}` must be a plain directory, not a symbolic link",
            state_dir.display()
        )));
    }
    let target = state_dir.join(subdir);
    match fs::symlink_metadata(&target) {
        Ok(meta) if meta.file_type().is_symlink() || !meta.is_dir() => {
            return Err(SiteError::InvalidFolder(format!(
                "`{}` must be a plain directory, not a symbolic link",
                target.display()
            )));
        }
        Ok(_) => {}
        Err(err) if err.kind() == io::ErrorKind::NotFound => {
            create_private_dir(&target)?;
        }
        Err(err) => return Err(err.into()),
    }
    let canonical_state = fs::canonicalize(state_dir)?;
    let canonical_target = fs::canonicalize(&target)?;
    if canonical_target != canonical_state.join(subdir) {
        return Err(SiteError::InvalidFolder(format!(
            "`{}` must be the plain `{subdir}` directory under state",
            target.display()
        )));
    }
    Ok(target)
}

pub(crate) fn create_private_dir(path: &Path) -> io::Result<()> {
    DirBuilder::new().mode(0o700).create(path)
}

/// Copies the tree at `src` into `dst`, then syncs what it wrote. Every
/// entry is opened through its parent's descriptor without following a
/// symbolic link, so a link swapped in while the copy runs is refused, never
/// followed; a FIFO or device is refused without blocking on it.
pub(super) fn copy_verified_tree(
    src: &Path,
    dst: &Path,
    stats: &mut CopyStats,
) -> Result<(), SiteError> {
    let root = Dir::open(src, DIRECTORY, Mode::empty()).map_err(|errno| match errno {
        Errno::ELOOP | Errno::ENOTDIR => {
            SiteError::InvalidFolder("the folder must be a real folder, not a link".to_owned())
        }
        other => io::Error::from(other).into(),
    })?;
    copy_dir(root, dst, stats)
}

const DIRECTORY: OFlag = OFlag::O_RDONLY
    .union(OFlag::O_DIRECTORY)
    .union(OFlag::O_NOFOLLOW)
    .union(OFlag::O_CLOEXEC);
const ENTRY: OFlag = OFlag::O_RDONLY
    .union(OFlag::O_NOFOLLOW)
    .union(OFlag::O_NONBLOCK)
    .union(OFlag::O_CLOEXEC);

fn copy_dir(mut dir: Dir, dst: &Path, stats: &mut CopyStats) -> Result<(), SiteError> {
    let mut names = Vec::new();
    for entry in dir.iter() {
        let name = entry.map_err(io::Error::from)?.file_name().to_owned();
        if name.as_bytes() != b"." && name.as_bytes() != b".." {
            names.push(name);
        }
    }
    names.sort();
    for name in names {
        stats.entries += 1;
        if stats.entries > MAX_SITE_ENTRIES {
            return Err(SiteError::TooManyEntries);
        }
        let shown = name.to_string_lossy().into_owned();
        let target = dst.join(OsStr::from_bytes(name.as_bytes()));
        let fd = openat(dir.as_fd(), name.as_c_str(), ENTRY, Mode::empty()).map_err(|errno| {
            if errno == Errno::ELOOP {
                SiteError::InvalidFolder(format!(
                    "`{shown}` is a symbolic link; static site folders must not contain symbolic links"
                ))
            } else {
                io::Error::from(errno).into()
            }
        })?;
        let kind =
            SFlag::from_bits_truncate(fstat(&fd).map_err(io::Error::from)?.st_mode) & SFlag::S_IFMT;
        if kind == SFlag::S_IFDIR {
            create_private_dir(&target)?;
            copy_dir(Dir::from_fd(fd).map_err(io::Error::from)?, &target, stats)?;
        } else if kind == SFlag::S_IFREG {
            stats.file_count += 1;
            copy_file(File::from(fd), &target, stats)?;
        } else {
            return Err(SiteError::InvalidFolder(format!(
                "`{shown}` is not a regular file or folder"
            )));
        }
    }
    File::open(dst)?.sync_all()?;
    Ok(())
}

/// Copies one file within what is left of the site's room, and syncs it.
fn copy_file(mut input: File, target: &Path, stats: &mut CopyStats) -> Result<(), SiteError> {
    let room = MAX_SITE_TOTAL_BYTES - stats.total_bytes;
    if input.metadata()?.len() > room {
        return Err(SiteError::SiteTooLarge);
    }
    let mut output = OpenOptions::new()
        .create_new(true)
        .write(true)
        .mode(0o400)
        .open(target)?;
    // A file that grows while it is copied stops one byte past the room.
    let copied = io::copy(&mut input.by_ref().take(room + 1), &mut output)?;
    if copied > room {
        return Err(SiteError::SiteTooLarge);
    }
    output.sync_all()?;
    stats.total_bytes += copied;
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
