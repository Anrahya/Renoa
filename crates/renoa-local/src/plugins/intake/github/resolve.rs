use serde::Deserialize;
use tokio_util::sync::CancellationToken;

use super::{http_client, invalid, unavailable, validate_commit};
use crate::plugins::{PluginError, intake::require_active};

const MAX_API_BYTES: usize = 1_048_576;
const MAX_REF_BYTES: usize = 256;

#[derive(Deserialize)]
struct CommitInfo {
    sha: String,
}

pub(in crate::plugins::intake) fn validate_ref(reference: &str) -> Result<(), PluginError> {
    if reference.is_empty()
        || reference.len() > MAX_REF_BYTES
        || reference == "."
        || reference.starts_with('/')
        || reference.ends_with('/')
        || reference.contains("//")
        || reference.contains("..")
        || reference.bytes().any(|byte| {
            byte.is_ascii_control()
                || byte.is_ascii_whitespace()
                || matches!(byte, b'?' | b'#' | b'\\' | b':')
        })
    {
        return Err(invalid(
            "GitHub ref must be a valid branch or tag name without whitespace or traversal; retry inspect with {\"action\":\"inspect\",\"source\":{\"kind\":\"github\",\"repository\":\"https://github.com/<owner>/<repo>\",\"ref\":\"<branch-or-tag>\"}}",
        ));
    }
    Ok(())
}

pub(in crate::plugins::intake) async fn resolve_commit(
    api_origin: &str,
    owner: &str,
    repo: &str,
    r#ref: Option<&str>,
    cancellation: CancellationToken,
) -> Result<String, PluginError> {
    require_active(&cancellation)?;
    let client = http_client("could not initialize GitHub API client")?;
    let target_ref = if let Some(reference) = r#ref {
        validate_ref(reference)?;
        reference
    } else {
        "HEAD"
    };
    let encoded_ref = encode_ref_segment(target_ref);
    let commit_url = format!("{api_origin}/repos/{owner}/{repo}/commits/{encoded_ref}");
    let repository = format!("https://github.com/{owner}/{repo}");
    let info: CommitInfo = get_json(
        &client,
        &commit_url,
        &repository,
        &format!("ref `{target_ref}`"),
        &cancellation,
    )
    .await?;
    validate_commit(&info.sha).map_err(|_| {
        invalid(&format!(
            "GitHub returned invalid commit SHA `{}` for ref `{target_ref}` in {repository}",
            info.sha
        ))
    })?;
    Ok(info.sha)
}

async fn get_json<T: for<'de> Deserialize<'de>>(
    client: &reqwest::Client,
    url: &str,
    repository: &str,
    subject: &str,
    cancellation: &CancellationToken,
) -> Result<T, PluginError> {
    require_active(cancellation)?;
    let mut response = tokio::select! {
        response = client
            .get(url)
            .header(reqwest::header::ACCEPT, "application/vnd.github+json")
            .send() => response.map_err(|_| unavailable("GitHub API request failed"))?,
        () = cancellation.cancelled() => return Err(PluginError::Cancelled),
    };
    let status = response.status();
    if status == reqwest::StatusCode::MOVED_PERMANENTLY || status.is_redirection() {
        return Err(invalid(&format!(
            "GitHub repository {repository} moved or redirected (HTTP {}); pass the canonical https://github.com/<owner>/<repo> URL and retry inspect with {{\"action\":\"inspect\",\"source\":{{\"kind\":\"github\",\"repository\":\"https://github.com/<owner>/<repo>\"}}}}",
            status.as_u16()
        )));
    }
    if status == reqwest::StatusCode::FORBIDDEN || status == reqwest::StatusCode::TOO_MANY_REQUESTS
    {
        return Err(unavailable(&format!(
            "GitHub API rate limit or access restriction for {repository} (HTTP {}); retry later or pass a pinned 40-character commit SHA directly with {{\"action\":\"inspect\",\"source\":{{\"kind\":\"github\",\"repository\":{repository:?},\"commit\":\"<40-char-sha>\"}}}}",
            status.as_u16()
        )));
    }
    if status == reqwest::StatusCode::NOT_FOUND {
        return Err(invalid(&format!(
            "GitHub {subject} was not found in {repository}; verify the repository and ref, then retry inspect with {{\"action\":\"inspect\",\"source\":{{\"kind\":\"github\",\"repository\":{repository:?}}}}}"
        )));
    }
    if !status.is_success() {
        return Err(unavailable(&format!(
            "GitHub API request for {subject} returned HTTP {}",
            status.as_u16()
        )));
    }
    if response
        .content_length()
        .is_some_and(|size| size > MAX_API_BYTES as u64)
    {
        return Err(invalid("GitHub API response exceeds 1 MiB"));
    }
    let mut body = Vec::new();
    loop {
        let chunk = tokio::select! {
            chunk = response.chunk() => chunk.map_err(|_| unavailable("GitHub API response ended unsuccessfully"))?,
            () = cancellation.cancelled() => return Err(PluginError::Cancelled),
        };
        let Some(chunk) = chunk else { break };
        if body.len().saturating_add(chunk.len()) > MAX_API_BYTES {
            return Err(invalid("GitHub API response exceeds 1 MiB"));
        }
        body.extend_from_slice(&chunk);
    }
    serde_json::from_slice(&body).map_err(|error| {
        invalid(&format!(
            "GitHub API response for {subject} is invalid JSON: {error}"
        ))
    })
}

fn encode_ref_segment(reference: &str) -> String {
    let mut encoded = String::with_capacity(reference.len());
    for byte in reference.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'.' | b'_' | b'~') {
            encoded.push(char::from(byte));
        } else {
            use std::fmt::Write as _;
            let _ = write!(encoded, "%{byte:02X}");
        }
    }
    encoded
}
