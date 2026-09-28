use std::{
    collections::BTreeSet,
    fs::{self, File, OpenOptions},
    io::{Read, Write},
    path::{Component, Path, PathBuf},
    time::Duration,
};

use flate2::read::GzDecoder;
use tempfile::TempDir;
use tokio_util::sync::CancellationToken;
use url::Url;

use super::{PluginError, require_active, scratch};

const MAX_ARCHIVE_BYTES: u64 = 128 * 1_024 * 1_024;
const MAX_ENTRIES: usize = 16_384;
const MAX_DEPTH: usize = 32;

pub(super) fn validate(
    repository: &str,
    commit: &str,
    path: Option<&str>,
) -> Result<(String, String), PluginError> {
    let url = Url::parse(repository)
        .map_err(|_| invalid("repository must be a canonical HTTPS GitHub repository URL"))?;
    let parts = url
        .path()
        .trim_start_matches('/')
        .split('/')
        .collect::<Vec<_>>();
    if url.scheme() != "https"
        || url.host_str() != Some("github.com")
        || url.port().is_some()
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
        || parts.len() != 2
        || parts.iter().any(|part| {
            part.is_empty()
                || *part == "."
                || *part == ".."
                || !part
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'))
        })
        || Path::new(parts[1])
            .extension()
            .is_some_and(|extension| extension.eq_ignore_ascii_case("git"))
    {
        return Err(invalid(
            "repository must be https://github.com/owner/repo without credentials, query, fragment, or .git suffix",
        ));
    }
    if commit.len() != 40
        || !commit
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return Err(invalid(
            "GitHub source requires a full 40-character lowercase hexadecimal commit SHA",
        ));
    }
    if let Some(path) = path {
        relative(path)?;
    }
    Ok((parts[0].to_owned(), parts[1].to_owned()))
}

#[derive(Clone)]
pub(crate) struct GithubSourceClient {
    origin: String,
}
impl Default for GithubSourceClient {
    fn default() -> Self {
        Self {
            origin: "https://codeload.github.com".to_owned(),
        }
    }
}
impl GithubSourceClient {
    pub(super) async fn download(
        &self,
        repository: &str,
        commit: &str,
        path: Option<&str>,
        cancellation: CancellationToken,
    ) -> Result<TempDir, PluginError> {
        let (owner, repo) = validate(repository, commit, path)?;
        let endpoint = format!("{}/{owner}/{repo}/tar.gz/{commit}", self.origin);
        download_from(&endpoint, &repo, commit, path, cancellation).await
    }
    #[cfg(test)]
    pub(crate) fn fixture(origin: String) -> Self {
        Self { origin }
    }
}

async fn download_from(
    endpoint: &str,
    repo: &str,
    commit: &str,
    path: Option<&str>,
    cancellation: CancellationToken,
) -> Result<TempDir, PluginError> {
    require_active(&cancellation)?;
    download_into(endpoint, repo, commit, path, cancellation, scratch()?).await
}

async fn download_into(
    endpoint: &str,
    repo: &str,
    commit: &str,
    path: Option<&str>,
    cancellation: CancellationToken,
    staging: TempDir,
) -> Result<TempDir, PluginError> {
    require_active(&cancellation)?;
    let client = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .timeout(Duration::from_secs(45))
        .build()
        .map_err(|_| unavailable("could not initialize GitHub download client"))?;
    let mut response = tokio::select! {
        response = client.get(endpoint).send() => response.map_err(|_|unavailable("GitHub source download failed"))?,
        () = cancellation.cancelled() => return Err(PluginError::Cancelled),
    };
    if !response.status().is_success() {
        return Err(unavailable(&format!(
            "GitHub source download returned HTTP {}",
            response.status().as_u16()
        )));
    }
    if response
        .content_length()
        .is_some_and(|size| size > MAX_ARCHIVE_BYTES)
    {
        return Err(invalid("GitHub source archive exceeds 128 MiB"));
    }
    let archive_path = staging.path().join("source.tar.gz");
    let mut file = tokio::fs::File::create(&archive_path)
        .await
        .map_err(|source| PluginError::Io {
            action: "create GitHub archive staging",
            path: archive_path.clone(),
            source,
        })?;
    let mut bytes = 0_u64;
    loop {
        let chunk = tokio::select! {
            chunk = response.chunk() => chunk.map_err(|_|unavailable("GitHub source download ended unsuccessfully"))?,
            () = cancellation.cancelled() => return Err(PluginError::Cancelled),
        };
        let Some(chunk) = chunk else { break };
        bytes = bytes
            .checked_add(chunk.len() as u64)
            .ok_or_else(|| invalid("GitHub source byte count overflowed"))?;
        if bytes > MAX_ARCHIVE_BYTES {
            return Err(invalid("GitHub source archive exceeds 128 MiB"));
        }
        tokio::io::AsyncWriteExt::write_all(&mut file, &chunk)
            .await
            .map_err(|source| PluginError::Io {
                action: "write GitHub archive",
                path: archive_path.clone(),
                source,
            })?;
    }
    // Tokio finishes a file's last write in the background; extraction must
    // not open the archive before that write lands.
    tokio::io::AsyncWriteExt::flush(&mut file)
        .await
        .map_err(|source| PluginError::Io {
            action: "write GitHub archive",
            path: archive_path.clone(),
            source,
        })?;
    drop(file);
    let prefix = format!("{repo}-{commit}");
    let subdirectory = path.map(str::to_owned);
    tokio::task::spawn_blocking(move || {
        extract(
            &archive_path,
            &staging.path().join("source"),
            &prefix,
            subdirectory.as_deref(),
            &cancellation,
        )?;
        fs::remove_file(&archive_path).map_err(|source| PluginError::Io {
            action: "remove downloaded GitHub archive",
            path: archive_path,
            source,
        })?;
        Ok(staging)
    })
    .await?
}

fn extract(
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

fn extract_limited(
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

struct BoundedReader<R> {
    reader: R,
    remaining: u64,
    cancellation: CancellationToken,
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

fn relative(value: &str) -> Result<(), PluginError> {
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

fn invalid(message: &str) -> PluginError {
    PluginError::Invalid(message.to_owned())
}
fn unavailable(message: &str) -> PluginError {
    PluginError::Unavailable(message.to_owned())
}

#[cfg(test)]
mod tests;
