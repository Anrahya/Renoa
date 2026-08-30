use std::{path::Path, time::Duration};

use futures_util::StreamExt as _;
use renoa_registry_protocol::{
    ARCHIVE_DIGEST_HEADER, ErrorResponse, PACKAGE_MEDIA_TYPE, PublishResult, RegistryChanges,
    RegistryStatus, Sha256Digest,
};
use reqwest::{StatusCode, Url, header};
use serde::de::DeserializeOwned;
use sha2::{Digest as _, Sha256};
use tokio::io::AsyncWriteExt as _;
use tokio_util::io::ReaderStream;

use super::{SharedRegistryError, archive::PackageArchive};

const MAX_JSON_BYTES: usize = 256 * 1_024;

#[derive(Clone)]
pub(super) struct RegistryClient {
    endpoint: Url,
    http: reqwest::Client,
}

impl RegistryClient {
    pub(super) fn new(endpoint: &str) -> Result<Self, SharedRegistryError> {
        let mut endpoint = Url::parse(endpoint).map_err(|error| {
            SharedRegistryError::Configuration(format!("shared registry URL is invalid: {error}"))
        })?;
        if !matches!(endpoint.scheme(), "http" | "https")
            || endpoint.cannot_be_a_base()
            || endpoint.host_str().is_none()
            || !endpoint.username().is_empty()
            || endpoint.password().is_some()
            || endpoint.query().is_some()
            || endpoint.fragment().is_some()
            || endpoint.path() != "/"
        {
            return Err(SharedRegistryError::Configuration(
                "shared registry URL must be an http(s) origin with no credentials, path, query, or fragment"
                    .to_owned(),
            ));
        }
        endpoint.set_path("/");
        let http = reqwest::Client::builder()
            .connect_timeout(Duration::from_secs(10))
            .timeout(Duration::from_mins(5))
            .redirect(reqwest::redirect::Policy::none())
            .no_proxy()
            .user_agent(concat!("renoa-host/", env!("CARGO_PKG_VERSION")))
            .build()?;
        Ok(Self { endpoint, http })
    }

    pub(super) async fn status(&self) -> Result<RegistryStatus, SharedRegistryError> {
        let response = self.http.get(self.url("v1/status")?).send().await?;
        read_json(response).await
    }

    pub(super) async fn contains(
        &self,
        package: &Sha256Digest,
    ) -> Result<bool, SharedRegistryError> {
        let response = self.http.head(self.package_url(package)?).send().await?;
        match response.status() {
            StatusCode::OK => Ok(true),
            StatusCode::NOT_FOUND => Ok(false),
            _ => Err(server_error(response).await),
        }
    }

    pub(super) async fn publish(
        &self,
        package: &Sha256Digest,
        archive: &PackageArchive,
    ) -> Result<PublishResult, SharedRegistryError> {
        let file = tokio::fs::File::open(archive.path()).await?;
        let response = self
            .http
            .put(self.package_url(package)?)
            .header(header::CONTENT_TYPE, PACKAGE_MEDIA_TYPE)
            .header(header::CONTENT_LENGTH, archive.bytes())
            .header(ARCHIVE_DIGEST_HEADER, archive.digest().as_str())
            .body(reqwest::Body::wrap_stream(ReaderStream::new(file)))
            .send()
            .await?;
        read_json(response).await
    }

    pub(super) async fn changes(&self, after: u64) -> Result<RegistryChanges, SharedRegistryError> {
        let mut url = self.url("v1/changes")?;
        url.query_pairs_mut()
            .append_pair("after", &after.to_string())
            .append_pair("limit", "100");
        let response = self.http.get(url).send().await?;
        read_json(response).await
    }

    pub(super) async fn download(
        &self,
        record: &renoa_registry_protocol::PackageRecord,
        transfer: &Path,
    ) -> Result<tempfile::TempPath, SharedRegistryError> {
        let response = self
            .http
            .get(self.package_url(record.package_digest())?)
            .send()
            .await?;
        if response.status() != StatusCode::OK {
            return Err(server_error(response).await);
        }
        require_download_headers(&response, record)?;
        let temporary = tempfile::Builder::new()
            .prefix("download-")
            .suffix(".tar")
            .tempfile_in(transfer)?;
        let path = temporary.into_temp_path();
        let mut file = tokio::fs::OpenOptions::new()
            .write(true)
            .truncate(true)
            .open(&path)
            .await?;
        let mut stream = response.bytes_stream();
        let mut bytes = 0_u64;
        let mut hasher = Sha256::new();
        while let Some(chunk) = stream.next().await {
            let chunk = chunk?;
            bytes = bytes
                .checked_add(u64::try_from(chunk.len()).map_err(|_| {
                    SharedRegistryError::Protocol("download byte count overflowed".to_owned())
                })?)
                .ok_or_else(|| {
                    SharedRegistryError::Protocol("download byte count overflowed".to_owned())
                })?;
            if bytes > record.archive_bytes() {
                return Err(SharedRegistryError::Protocol(
                    "download exceeded its declared package size".to_owned(),
                ));
            }
            hasher.update(&chunk);
            file.write_all(&chunk).await?;
        }
        file.sync_all().await?;
        drop(file);
        let digest = Sha256Digest::parse(hex(&hasher.finalize()))
            .map_err(|error| SharedRegistryError::Protocol(error.to_string()))?;
        if bytes != record.archive_bytes() || &digest != record.archive_digest() {
            return Err(SharedRegistryError::Conflict(
                "downloaded package archive differs from its registry record".to_owned(),
            ));
        }
        Ok(path)
    }

    fn package_url(&self, package: &Sha256Digest) -> Result<Url, SharedRegistryError> {
        self.url(&format!("v1/packages/{}", package.as_str()))
    }

    fn url(&self, relative: &str) -> Result<Url, SharedRegistryError> {
        self.endpoint
            .join(relative)
            .map_err(|error| SharedRegistryError::Configuration(error.to_string()))
    }
}

async fn read_json<T: DeserializeOwned>(
    response: reqwest::Response,
) -> Result<T, SharedRegistryError> {
    if !response.status().is_success() {
        return Err(server_error(response).await);
    }
    let content_type = response
        .headers()
        .get(header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok());
    if !content_type.is_some_and(|value| value.starts_with("application/json")) {
        return Err(SharedRegistryError::Protocol(
            "shared registry JSON response has the wrong content type".to_owned(),
        ));
    }
    let bytes = bounded_body(response, MAX_JSON_BYTES).await?;
    serde_json::from_slice(&bytes).map_err(SharedRegistryError::from)
}

async fn server_error(response: reqwest::Response) -> SharedRegistryError {
    let status = response.status();
    match bounded_body(response, 64 * 1_024).await {
        Ok(bytes) => match serde_json::from_slice::<ErrorResponse>(&bytes) {
            Ok(error) => SharedRegistryError::Server {
                status: status.as_u16(),
                code: error.code().to_owned(),
                message: error.message().to_owned(),
            },
            Err(_) => SharedRegistryError::Server {
                status: status.as_u16(),
                code: "invalid_error".to_owned(),
                message: "shared registry returned an invalid error response".to_owned(),
            },
        },
        Err(error) => error,
    }
}

async fn bounded_body(
    response: reqwest::Response,
    limit: usize,
) -> Result<Vec<u8>, SharedRegistryError> {
    let mut stream = response.bytes_stream();
    let mut body = Vec::new();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk?;
        if body.len().saturating_add(chunk.len()) > limit {
            return Err(SharedRegistryError::Protocol(format!(
                "shared registry JSON exceeds {limit} bytes"
            )));
        }
        body.extend_from_slice(&chunk);
    }
    Ok(body)
}

fn require_download_headers(
    response: &reqwest::Response,
    record: &renoa_registry_protocol::PackageRecord,
) -> Result<(), SharedRegistryError> {
    let content_type = response
        .headers()
        .get(header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok());
    let content_length = response.content_length();
    let digest = response
        .headers()
        .get(ARCHIVE_DIGEST_HEADER)
        .and_then(|value| value.to_str().ok());
    if content_type != Some(PACKAGE_MEDIA_TYPE)
        || content_length != Some(record.archive_bytes())
        || digest != Some(record.archive_digest().as_str())
    {
        return Err(SharedRegistryError::Protocol(
            "package response headers differ from the registry record".to_owned(),
        ));
    }
    Ok(())
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
