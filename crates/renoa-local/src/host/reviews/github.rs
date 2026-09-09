use std::time::Duration;

use reqwest::{
    Client, Method,
    header::{ACCEPT, AUTHORIZATION, HeaderMap, HeaderValue},
};
use serde::{Deserialize, de::DeserializeOwned};
use tokio_util::sync::CancellationToken;
use url::Url;

use super::{GitHubReviewError, GitHubReviewPolicy};

/// Credentials stay in the deterministic adapter; model tools accept only
/// repository-relative paths, never URLs, headers or arbitrary API methods.
#[derive(Clone)]
pub(super) struct GitHub {
    client: Client,
    origin: Url,
    headers: HeaderMap,
    pub(super) repository: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
pub(super) struct Pull {
    pub number: i64,
    pub state: String,
    pub draft: bool,
    pub title: String,
    pub body: Option<String>,
    pub base: Revision,
    pub head: Revision,
    pub changed_files: usize,
}
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
pub(super) struct Revision {
    pub sha: String,
    pub repo: Option<Repository>,
}
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
pub(super) struct Repository {
    pub id: i64,
    pub full_name: String,
}

impl GitHub {
    pub(super) fn installation_token(&self) -> std::io::Result<&str> {
        self.headers
            .get(AUTHORIZATION)
            .and_then(|header| header.to_str().ok())
            .and_then(|value| value.strip_prefix("Bearer "))
            .ok_or_else(|| std::io::Error::other("installation authentication unavailable"))
    }
    pub(super) async fn connect(
        origin: Url,
        app_jwt: &str,
        policy: &GitHubReviewPolicy,
        cancel: &CancellationToken,
    ) -> Result<Self, GitHubReviewError> {
        Self::connect_with_permissions(origin, app_jwt, policy, false, cancel).await
    }

    pub(super) async fn connect_with_permissions(
        origin: Url,
        app_jwt: &str,
        policy: &GitHubReviewPolicy,
        publish: bool,
        cancel: &CancellationToken,
    ) -> Result<Self, GitHubReviewError> {
        #[derive(Deserialize)]
        struct Installation {
            id: i64,
        }
        #[derive(Deserialize)]
        struct Token {
            token: String,
        }
        let client = Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .connect_timeout(Duration::from_secs(10))
            .timeout(Duration::from_secs(30))
            .user_agent("Renoa-GitHub-Reviewer/1")
            .build()?;
        let mut github = Self {
            client,
            origin,
            headers: headers(app_jwt)?,
            repository: policy.full_name.clone(),
        };
        let install: Installation = github.repo_json(&["installation"], &[], cancel).await?;
        if install.id != policy.installation_id {
            return Err(GitHubReviewError::Authentication);
        }
        let token: Token = serde_json::from_slice(&github.request(
            Method::POST, &["app", "installations", &install.id.to_string(), "access_tokens"], &[],
            Some(serde_json::to_vec(&serde_json::json!({"repository_ids":[policy.repository_id],"permissions":{"contents":"read","pull_requests":if publish {"write"} else {"read"},"checks":"read"}}))?),
            false, cancel,
        ).await?)?;
        github.headers = headers(&token.token)?;
        let repository: Repository = github.repo_json(&[], &[], cancel).await?;
        if repository.id != policy.repository_id || repository.full_name != policy.full_name {
            return Err(GitHubReviewError::Authentication);
        }
        Ok(github)
    }

    pub(super) async fn pull(
        &self,
        number: i64,
        cancel: &CancellationToken,
    ) -> Result<Pull, GitHubReviewError> {
        let pull: Pull = self
            .repo_json(&["pulls", &number.to_string()], &[], cancel)
            .await?;
        super::validate_target(pull.number, &pull.base.sha, &pull.head.sha)?;
        if pull.number != number || !matches!(pull.state.as_str(), "open" | "closed") {
            return Err(GitHubReviewError::Invalid(
                "invalid GitHub PR identity/state".to_owned(),
            ));
        }
        Ok(pull)
    }

    pub(super) async fn merge_base(
        &self,
        pull: &Pull,
        cancel: &CancellationToken,
    ) -> Result<String, GitHubReviewError> {
        #[derive(Deserialize)]
        struct Comparison {
            merge_base_commit: Commit,
        }
        #[derive(Deserialize)]
        struct Commit {
            sha: String,
        }
        let comparison: Comparison = self
            .repo_json(
                &["compare", &format!("{}...{}", pull.base.sha, pull.head.sha)],
                // GitHub includes the comparison's file patches only on page
                // one, even with per_page=1. Later pages retain merge-base
                // metadata even when there are no more commits to list.
                &[("per_page", "1"), ("page", "2")],
                cancel,
            )
            .await?;
        super::validate_target(
            pull.number,
            &comparison.merge_base_commit.sha,
            &pull.head.sha,
        )?;
        Ok(comparison.merge_base_commit.sha)
    }

    pub(super) async fn repo_json<T: DeserializeOwned>(
        &self,
        path: &[&str],
        query: &[(&str, &str)],
        cancel: &CancellationToken,
    ) -> Result<T, GitHubReviewError> {
        let parts: Vec<_> = std::iter::once("repos")
            .chain(self.repository.split('/'))
            .chain(path.iter().copied())
            .collect();
        Ok(serde_json::from_slice(
            &self
                .request(Method::GET, &parts, query, None, false, cancel)
                .await?,
        )?)
    }

    #[cfg(test)]
    pub(super) async fn source(
        &self,
        path: &str,
        sha: &str,
        cancel: &CancellationToken,
    ) -> Result<String, GitHubReviewError> {
        valid_path(path)?;
        let parts: Vec<_> = std::iter::once("repos")
            .chain(self.repository.split('/'))
            .chain(std::iter::once("contents"))
            .chain(path.split('/'))
            .collect();
        let bytes = self
            .request(Method::GET, &parts, &[("ref", sha)], None, true, cancel)
            .await?;
        String::from_utf8(bytes)
            .map_err(|_| GitHubReviewError::Invalid("source is not UTF-8 text".to_owned()))
    }

    pub(super) async fn request(
        &self,
        method: Method,
        path: &[&str],
        query: &[(&str, &str)],
        body: Option<Vec<u8>>,
        raw: bool,
        cancel: &CancellationToken,
    ) -> Result<Vec<u8>, GitHubReviewError> {
        let limit = if raw { 64 * 1024_usize } else { 1024 * 1024 };
        let mut url = self.origin.clone();
        url.path_segments_mut()
            .map_err(|()| GitHubReviewError::Invalid("invalid GitHub API origin".to_owned()))?
            .clear()
            .extend(path);
        url.query_pairs_mut().extend_pairs(query.iter().copied());
        let mut headers = self.headers.clone();
        if raw {
            headers.insert(
                ACCEPT,
                HeaderValue::from_static("application/vnd.github.raw+json"),
            );
        }
        let mut request = self.client.request(method, url).headers(headers);
        if let Some(body) = body {
            request = request
                .header("content-type", "application/json")
                .body(body);
        }
        let work = async {
            let mut response = request.send().await?;
            if !response.status().is_success() {
                return Err(GitHubReviewError::Api {
                    status: response.status().as_u16(),
                    retry_after: retry_header(response.headers()),
                });
            }
            let mut bytes = Vec::new();
            while let Some(chunk) = response.chunk().await? {
                if chunk.len() > limit.saturating_sub(bytes.len()) {
                    return Err(GitHubReviewError::ContextLimit);
                }
                bytes.extend_from_slice(&chunk);
            }
            Ok(bytes)
        };
        tokio::select! { biased; ()=cancel.cancelled()=>Err(GitHubReviewError::Cancelled), result=work=>result }
    }
}

fn retry_header(headers: &HeaderMap) -> Option<String> {
    headers
        .get("retry-after")
        .and_then(|v| v.to_str().ok())
        .map(str::to_owned)
        .or_else(|| {
            if headers.get("x-ratelimit-remaining")?.to_str().ok()? != "0" {
                return None;
            }
            let reset: i64 = headers
                .get("x-ratelimit-reset")?
                .to_str()
                .ok()?
                .parse()
                .ok()?;
            let now = crate::TurnObservation::now().ok()?.unix_milliseconds() / 1000;
            Some(reset.saturating_sub(now).max(0).to_string())
        })
}

fn headers(token: &str) -> Result<HeaderMap, GitHubReviewError> {
    if token.is_empty() || token.len() > 16 * 1024 {
        return Err(GitHubReviewError::Authentication);
    }
    let mut auth = HeaderValue::from_str(&format!("Bearer {token}"))
        .map_err(|_| GitHubReviewError::Authentication)?;
    auth.set_sensitive(true);
    let mut headers = HeaderMap::new();
    headers.insert(AUTHORIZATION, auth);
    headers.insert(
        ACCEPT,
        HeaderValue::from_static("application/vnd.github+json"),
    );
    headers.insert(
        "x-github-api-version",
        HeaderValue::from_static("2022-11-28"),
    );
    Ok(headers)
}

#[cfg(test)]
pub(super) fn valid_path(path: &str) -> Result<(), GitHubReviewError> {
    if path.len() > 1024
        || path
            .split('/')
            .any(|part| part.is_empty() || matches!(part, "." | ".."))
        || path.chars().any(|ch| ch.is_control() || ch == '\\')
    {
        return Err(GitHubReviewError::Invalid(
            "invalid repository-relative source path".to_owned(),
        ));
    }
    Ok(())
}
