use std::{
    fs,
    path::{Path, PathBuf},
    time::Duration,
};

use tempfile::TempDir;
use tokio_util::sync::CancellationToken;
use url::Url;

use super::{PluginError, require_active, scratch};

const MAX_ARCHIVE_BYTES: u64 = 128 * 1_024 * 1_024;
const MAX_ENTRIES: usize = 16_384;
pub(super) const MAX_DEPTH: usize = 32;

mod archive;
mod resolve;

#[cfg(test)]
use archive::{BoundedReader, extract, extract_limited};
use archive::{extract_with_policy, relative};
pub(super) use resolve::{resolve_commit, validate_ref};

pub(super) fn is_valid_segment(segment: &str) -> bool {
    !segment.is_empty()
        && segment != "."
        && segment != ".."
        && segment
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'))
}

pub(super) fn validate_locator(
    repository: &str,
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
        || parts.iter().any(|part| !is_valid_segment(part))
        || Path::new(parts[1])
            .extension()
            .is_some_and(|extension| extension.eq_ignore_ascii_case("git"))
    {
        return Err(invalid(
            "repository must be https://github.com/owner/repo without credentials, query, fragment, or .git suffix",
        ));
    }
    if let Some(path) = path {
        relative(path)?;
    }
    Ok((parts[0].to_owned(), parts[1].to_owned()))
}

pub(super) fn validate_commit(commit: &str) -> Result<(), PluginError> {
    if commit.len() != 40
        || !commit
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return Err(invalid(
            "GitHub source requires a full 40-character lowercase hexadecimal commit SHA",
        ));
    }
    Ok(())
}

pub(super) fn validate(
    repository: &str,
    commit: &str,
    path: Option<&str>,
) -> Result<(String, String), PluginError> {
    let locator = validate_locator(repository, path)?;
    validate_commit(commit)?;
    Ok(locator)
}

#[derive(Clone)]
pub(crate) struct GithubSourceClient {
    origin: String,
    api_origin: String,
}
impl Default for GithubSourceClient {
    fn default() -> Self {
        Self {
            origin: "https://codeload.github.com".to_owned(),
            api_origin: "https://api.github.com".to_owned(),
        }
    }
}
pub(super) struct DownloadedSource {
    pub(super) staging: TempDir,
    pub(super) skipped_special: Vec<PathBuf>,
}

impl GithubSourceClient {
    pub(super) async fn resolve_commit(
        &self,
        repository: &str,
        r#ref: Option<&str>,
        cancellation: CancellationToken,
    ) -> Result<String, PluginError> {
        let (owner, repo) = validate_locator(repository, None)?;
        resolve_commit(&self.api_origin, &owner, &repo, r#ref, cancellation).await
    }

    pub(super) async fn download_with_policy(
        &self,
        repository: &str,
        commit: &str,
        path: Option<&str>,
        tolerate_unrelated_links: bool,
        cancellation: CancellationToken,
    ) -> Result<DownloadedSource, PluginError> {
        let (owner, repo) = validate(repository, commit, path)?;
        let endpoint = format!("{}/{owner}/{repo}/tar.gz/{commit}", self.origin);
        download_into_with_policy(
            &endpoint,
            &repo,
            commit,
            path,
            tolerate_unrelated_links,
            cancellation,
            scratch()?,
        )
        .await
    }
    #[cfg(test)]
    pub(crate) fn fixture(origin: String) -> Self {
        Self {
            origin: origin.clone(),
            api_origin: origin,
        }
    }
}

pub(super) fn http_client(action: &str) -> Result<reqwest::Client, PluginError> {
    reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .timeout(Duration::from_secs(45))
        .user_agent("renoa")
        .build()
        .map_err(|_| unavailable(action))
}

#[cfg(test)]
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

#[cfg(test)]
async fn download_into(
    endpoint: &str,
    repo: &str,
    commit: &str,
    path: Option<&str>,
    cancellation: CancellationToken,
    staging: TempDir,
) -> Result<TempDir, PluginError> {
    download_into_with_policy(endpoint, repo, commit, path, false, cancellation, staging)
        .await
        .map(|downloaded| downloaded.staging)
}

async fn download_into_with_policy(
    endpoint: &str,
    repo: &str,
    commit: &str,
    path: Option<&str>,
    tolerate_unrelated_links: bool,
    cancellation: CancellationToken,
    staging: TempDir,
) -> Result<DownloadedSource, PluginError> {
    require_active(&cancellation)?;
    let client = http_client("could not initialize GitHub download client")?;
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
        return Err(invalid(
            "GitHub source archive exceeds 128 MiB; pass `path` in inspect or select a smaller repository",
        ));
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
            return Err(invalid(
                "GitHub source archive exceeds 128 MiB; pass `path` in inspect or select a smaller repository",
            ));
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
        let skipped_special = extract_with_policy(
            &archive_path,
            &staging.path().join("source"),
            &prefix,
            subdirectory.as_deref(),
            &cancellation,
            tolerate_unrelated_links,
        )?;
        fs::remove_file(&archive_path).map_err(|source| PluginError::Io {
            action: "remove downloaded GitHub archive",
            path: archive_path,
            source,
        })?;
        Ok(DownloadedSource {
            staging,
            skipped_special,
        })
    })
    .await?
}

fn invalid(message: &str) -> PluginError {
    PluginError::Invalid(message.to_owned())
}
fn unavailable(message: &str) -> PluginError {
    PluginError::Unavailable(message.to_owned())
}

#[cfg(test)]
mod tests;
