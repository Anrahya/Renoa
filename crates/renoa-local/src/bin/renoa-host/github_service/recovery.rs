//! Reconcile GitHub's retained delivery history with durable Host receipts.
use super::auth::AppAuth;
use renoa_local::{GitHubReviewError, LocalHost, TurnObservation};
use reqwest::{Client, Method, header::HeaderMap};
use serde::Deserialize;
use std::{collections::HashSet, error::Error, path::Path, time::Duration};
use tokio_util::sync::CancellationToken;
use url::Url;
use uuid::Uuid;

#[cfg(test)]
#[path = "recovery_tests.rs"]
mod tests;

const INTERVAL_MS: i64 = 5 * 60 * 1000;

pub(super) async fn run(
    host: &LocalHost,
    data: &Path,
    auth: &AppAuth,
    stop: &CancellationToken,
) -> Result<(), Box<dyn Error>> {
    let path = data.join(".github-recovery.json");
    let mut next = match tokio::fs::read(&path).await {
        Ok(bytes) => serde_json::from_slice::<i64>(&bytes)?,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => 0,
        Err(e) => return Err(e.into()),
    };
    loop {
        let now = TurnObservation::now()?.unix_milliseconds();
        if next > now {
            tokio::select! {
                ()=stop.cancelled()=>return Ok(()),
                ()=tokio::time::sleep(Duration::from_millis(next.saturating_sub(now).unsigned_abs()))=>{}
            }
        }
        if stop.is_cancelled() {
            return Ok(());
        }
        let now = TurnObservation::now()?.unix_milliseconds();
        next = now.saturating_add(INTERVAL_MS);
        // Commit the next scan before any request, so a restart cannot create
        // a redelivery storm. Re-scanning all retained pages avoids ordering
        // assumptions about original deliveries and later redelivery attempts.
        save_next(&path, next).await?;
        let api = Api::new(Url::parse("https://api.github.com")?, &auth.jwt()?)?;
        if let Err(error) = api.scan(host, now, stop).await {
            if matches!(error, GitHubReviewError::Cancelled) {
                return Ok(());
            }
            next = retry_at(&error, now).max(next);
            save_next(&path, next).await?;
            eprintln!("GitHub delivery recovery: {error}; next scan at {next}");
        }
    }
}

async fn save_next(path: &Path, next: i64) -> Result<(), std::io::Error> {
    let path = path.to_owned();
    tokio::task::spawn_blocking(move || {
        use std::io::Write as _;
        let parent = path
            .parent()
            .ok_or_else(|| std::io::Error::other("missing state directory"))?;
        let mut file = tempfile::NamedTempFile::new_in(parent)?;
        file.write_all(next.to_string().as_bytes())?;
        file.as_file().sync_all()?;
        file.persist(&path).map_err(|e| e.error)?;
        std::fs::File::open(parent)?.sync_all()
    })
    .await
    .map_err(std::io::Error::other)?
}

pub(super) fn retry_at(error: &GitHubReviewError, now: i64) -> i64 {
    let requested = match error {
        GitHubReviewError::Api {
            retry_after: Some(value),
            ..
        } => value
            .parse::<i64>()
            .ok()
            .filter(|v| *v >= 0)
            .map(|seconds| now.saturating_add(seconds.saturating_mul(1000)))
            .or_else(|| {
                jiff::fmt::rfc2822::parse(value)
                    .ok()
                    .map(|date| date.timestamp().as_millisecond())
            }),
        _ => None,
    };
    requested.unwrap_or(0).max(now.saturating_add(INTERVAL_MS))
}

struct Api {
    client: Client,
    origin: Url,
}

#[derive(Deserialize)]
struct Delivery {
    id: u64,
    guid: Uuid,
    event: String,
    status_code: u16,
    delivered_at: String,
}

impl Api {
    fn new(origin: Url, jwt: &str) -> Result<Self, GitHubReviewError> {
        let mut headers = HeaderMap::new();
        let mut auth = reqwest::header::HeaderValue::from_str(&format!("Bearer {jwt}"))
            .map_err(|_| GitHubReviewError::Authentication)?;
        auth.set_sensitive(true);
        headers.insert(reqwest::header::AUTHORIZATION, auth);
        headers.insert(
            "x-github-api-version",
            reqwest::header::HeaderValue::from_static("2022-11-28"),
        );
        headers.insert(
            reqwest::header::ACCEPT,
            reqwest::header::HeaderValue::from_static("application/vnd.github+json"),
        );
        Ok(Self {
            client: Client::builder()
                .default_headers(headers)
                .redirect(reqwest::redirect::Policy::none())
                .connect_timeout(Duration::from_secs(10))
                .timeout(Duration::from_secs(30))
                .user_agent("Renoa-GitHub-Recovery/1")
                .build()?,
            origin,
        })
    }

    async fn scan(
        &self,
        host: &LocalHost,
        now: i64,
        stop: &CancellationToken,
    ) -> Result<(), GitHubReviewError> {
        let mut next = Some(
            self.origin
                .join("/app/hook/deliveries?per_page=100")
                .map_err(|e| GitHubReviewError::Invalid(e.to_string()))?,
        );
        let mut visited = HashSet::new();
        let mut attempted = HashSet::new();
        while let Some(url) = next {
            if !visited.insert(url.clone()) {
                return Err(GitHubReviewError::Invalid(
                    "GitHub repeated a delivery page".to_owned(),
                ));
            }
            let (headers, body) = self.request(Method::GET, url, stop).await?;
            let deliveries: Vec<Delivery> = serde_json::from_slice(&body)?;
            for delivery in deliveries {
                if delivery.event != "pull_request" || (200..400).contains(&delivery.status_code) {
                    continue;
                }
                let time = delivery
                    .delivered_at
                    .parse::<jiff::Timestamp>()
                    .map_err(|e| GitHubReviewError::Invalid(e.to_string()))?
                    .as_millisecond();
                // A zero status can also mean an in-flight attempt. Give it time
                // to finish before asking GitHub for the same signed delivery.
                if time > now.saturating_sub(60_000) || !attempted.insert(delivery.guid) {
                    continue;
                }
                if host
                    .has_github_delivery(delivery.guid)
                    .await
                    .map_err(|e| GitHubReviewError::Invalid(e.to_string()))?
                {
                    continue;
                }
                let url = self
                    .origin
                    .join(&format!("/app/hook/deliveries/{}/attempts", delivery.id))
                    .map_err(|e| GitHubReviewError::Invalid(e.to_string()))?;
                self.request(Method::POST, url, stop).await?;
                eprintln!(
                    "Requested GitHub redelivery {} ({})",
                    delivery.guid, delivery.id
                );
            }
            next = next_page(&headers, &self.origin)?;
        }
        Ok(())
    }

    async fn request(
        &self,
        method: Method,
        url: Url,
        stop: &CancellationToken,
    ) -> Result<(HeaderMap, Vec<u8>), GitHubReviewError> {
        let work = async {
            let mut response = self.client.request(method, url).send().await?;
            if !response.status().is_success() {
                return Err(GitHubReviewError::Api {
                    status: response.status().as_u16(),
                    retry_after: retry_header(response.headers()),
                });
            }
            let headers = response.headers().clone();
            let mut body = Vec::new();
            while let Some(chunk) = response.chunk().await? {
                if chunk.len() > (1024 * 1024_usize).saturating_sub(body.len()) {
                    return Err(GitHubReviewError::ContextLimit);
                }
                body.extend_from_slice(&chunk);
            }
            Ok((headers, body))
        };
        tokio::select! { ()=stop.cancelled()=>Err(GitHubReviewError::Cancelled), result=work=>result }
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
            let now = TurnObservation::now().ok()?.unix_milliseconds() / 1000;
            Some(reset.saturating_sub(now).max(0).to_string())
        })
}

fn next_page(headers: &HeaderMap, origin: &Url) -> Result<Option<Url>, GitHubReviewError> {
    for header in headers.get_all("link") {
        let value = header
            .to_str()
            .map_err(|e| GitHubReviewError::Invalid(e.to_string()))?;
        for part in value.split(',') {
            let mut parts = part.trim().split(';');
            let target = parts.next().unwrap_or_default();
            if !parts.any(|p| p.trim() == "rel=\"next\"") {
                continue;
            }
            let target = target
                .strip_prefix('<')
                .and_then(|t| t.strip_suffix('>'))
                .ok_or_else(|| {
                    GitHubReviewError::Invalid("invalid GitHub pagination link".to_owned())
                })?;
            let url = Url::parse(target).map_err(|e| GitHubReviewError::Invalid(e.to_string()))?;
            if url.origin() != origin.origin()
                || url.path() != "/app/hook/deliveries"
                || !url.username().is_empty()
                || url.password().is_some()
                || url.fragment().is_some()
            {
                return Err(GitHubReviewError::Invalid(
                    "GitHub pagination left the delivery endpoint".to_owned(),
                ));
            }
            return Ok(Some(url));
        }
    }
    Ok(None)
}
