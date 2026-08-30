use std::{path::Path, sync::Arc, time::Duration};

use axum::{
    Json, Router,
    body::Body,
    extract::{Path as RoutePath, Query, State},
    http::{HeaderMap, StatusCode, header},
    response::{IntoResponse, Response},
    routing::get,
};
use futures_util::StreamExt as _;
use renoa_registry_protocol::{
    ARCHIVE_DIGEST_HEADER, ErrorResponse, MAX_PACKAGE_ARCHIVE_BYTES, PACKAGE_MEDIA_TYPE,
    PublishResult, RegistryChanges, RegistryStatus, Sha256Digest,
};
use serde::Deserialize;
use sha2::{Digest as _, Sha256};
use tokio::{io::AsyncWriteExt as _, net::TcpListener};
use tokio_util::{io::ReaderStream, sync::CancellationToken};

use crate::store::{RegistryError, RegistryStore};

#[derive(Clone)]
pub struct Registry {
    store: RegistryStore,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ChangesQuery {
    #[serde(default)]
    after: u64,
    #[serde(default = "default_change_limit")]
    limit: usize,
}

impl Registry {
    /// Opens or creates one durable registry state directory.
    ///
    /// # Errors
    ///
    /// Returns when its owner-only directories, `SQLite` state, or package blobs are invalid.
    pub fn open(root: impl AsRef<Path>) -> Result<Self, RegistryError> {
        Ok(Self {
            store: RegistryStore::open(root.as_ref())?,
        })
    }

    /// Serves the registry API on an already-bound listener until cancellation.
    ///
    /// # Errors
    ///
    /// Returns when the HTTP server cannot serve or shut down cleanly.
    pub async fn serve(
        self,
        listener: TcpListener,
        shutdown: CancellationToken,
    ) -> Result<(), RegistryError> {
        let router = Router::new()
            .route("/v1/status", get(status))
            .route("/v1/changes", get(changes))
            .route(
                "/v1/packages/{digest}",
                get(download).head(package_exists).put(upload),
            )
            .with_state(Arc::new(self.store));
        axum::serve(listener, router)
            .with_graceful_shutdown(shutdown.cancelled_owned())
            .await
            .map_err(RegistryError::Io)
    }
}

async fn status(State(store): State<Arc<RegistryStore>>) -> Result<Json<RegistryStatus>, ApiError> {
    blocking(move || store.status()).await.map(Json)
}

async fn changes(
    State(store): State<Arc<RegistryStore>>,
    Query(query): Query<ChangesQuery>,
) -> Result<Json<RegistryChanges>, ApiError> {
    blocking(move || store.changes(query.after, query.limit))
        .await
        .map(Json)
}

async fn package_exists(
    State(store): State<Arc<RegistryStore>>,
    RoutePath(digest): RoutePath<String>,
) -> Result<StatusCode, ApiError> {
    let digest =
        Sha256Digest::parse(digest).map_err(|error| ApiError::invalid(error.to_string()))?;
    blocking(move || {
        let record = store.package(&digest)?;
        store.verified_blob(&record)?;
        Ok(())
    })
    .await?;
    Ok(StatusCode::OK)
}

async fn download(
    State(store): State<Arc<RegistryStore>>,
    RoutePath(digest): RoutePath<String>,
) -> Result<Response, ApiError> {
    let digest =
        Sha256Digest::parse(digest).map_err(|error| ApiError::invalid(error.to_string()))?;
    let record = blocking({
        let store = Arc::clone(&store);
        move || store.package(&digest)
    })
    .await?;
    let path = blocking({
        let store = Arc::clone(&store);
        let record = record.clone();
        move || store.verified_blob(&record)
    })
    .await?;
    let file = tokio::fs::File::open(path)
        .await
        .map_err(ApiError::internal)?;
    Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, PACKAGE_MEDIA_TYPE)
        .header(header::CONTENT_LENGTH, record.archive_bytes())
        .header(ARCHIVE_DIGEST_HEADER, record.archive_digest().as_str())
        .body(Body::from_stream(ReaderStream::new(file)))
        .map_err(ApiError::internal)
}

async fn upload(
    State(store): State<Arc<RegistryStore>>,
    RoutePath(package): RoutePath<String>,
    headers: HeaderMap,
    body: Body,
) -> Result<Json<PublishResult>, ApiError> {
    let package =
        Sha256Digest::parse(package).map_err(|error| ApiError::invalid(error.to_string()))?;
    require_media_type(&headers)?;
    let expected_bytes = content_length(&headers)?;
    let expected_archive = header_digest(&headers)?;
    let staging = store.staging_path();
    let Ok(received) =
        tokio::time::timeout(Duration::from_mins(5), receive_archive(body, &staging)).await
    else {
        discard(&staging).await;
        return Err(ApiError::timeout());
    };
    let (observed_archive, observed_bytes) = match received {
        Ok(received) => received,
        Err(error) => {
            discard(&staging).await;
            return Err(error);
        }
    };
    if observed_bytes != expected_bytes || observed_archive != expected_archive {
        discard(&staging).await;
        return Err(ApiError::invalid(
            "archive body differs from its declared length or digest",
        ));
    }
    let result = blocking({
        let store = Arc::clone(&store);
        let staging = staging.clone();
        move || store.publish(&staging, &package, &observed_archive, observed_bytes)
    })
    .await;
    if result.is_err() {
        discard(&staging).await;
    }
    result.map(Json)
}

async fn receive_archive(body: Body, path: &Path) -> Result<(Sha256Digest, u64), ApiError> {
    let mut file = tokio::fs::OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(path)
        .await
        .map_err(ApiError::internal)?;
    let mut stream = body.into_data_stream();
    let mut bytes = 0_u64;
    let mut hasher = Sha256::new();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(ApiError::invalid)?;
        bytes = bytes
            .checked_add(u64::try_from(chunk.len()).map_err(ApiError::invalid)?)
            .ok_or_else(|| ApiError::invalid("archive byte count overflowed"))?;
        if bytes > MAX_PACKAGE_ARCHIVE_BYTES {
            return Err(ApiError::invalid(format!(
                "archive exceeds {MAX_PACKAGE_ARCHIVE_BYTES} bytes"
            )));
        }
        hasher.update(&chunk);
        file.write_all(&chunk).await.map_err(ApiError::internal)?;
    }
    if bytes == 0 {
        return Err(ApiError::invalid("archive body must not be empty"));
    }
    file.sync_all().await.map_err(ApiError::internal)?;
    drop(file);
    let digest = Sha256Digest::parse(hex(&hasher.finalize()))
        .map_err(|error| ApiError::internal(error.to_string()))?;
    Ok((digest, bytes))
}

fn require_media_type(headers: &HeaderMap) -> Result<(), ApiError> {
    let media_type = headers
        .get(header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok());
    if media_type == Some(PACKAGE_MEDIA_TYPE) {
        Ok(())
    } else {
        Err(ApiError::invalid(format!(
            "content-type must be {PACKAGE_MEDIA_TYPE}"
        )))
    }
}

fn content_length(headers: &HeaderMap) -> Result<u64, ApiError> {
    let length = headers
        .get(header::CONTENT_LENGTH)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.parse::<u64>().ok())
        .ok_or_else(|| ApiError::invalid("a valid content-length header is required"))?;
    if (1..=MAX_PACKAGE_ARCHIVE_BYTES).contains(&length) {
        Ok(length)
    } else {
        Err(ApiError::invalid(format!(
            "content-length must be between 1 and {MAX_PACKAGE_ARCHIVE_BYTES}"
        )))
    }
}

fn header_digest(headers: &HeaderMap) -> Result<Sha256Digest, ApiError> {
    let value = headers
        .get(ARCHIVE_DIGEST_HEADER)
        .and_then(|value| value.to_str().ok())
        .ok_or_else(|| ApiError::invalid(format!("{ARCHIVE_DIGEST_HEADER} is required")))?;
    Sha256Digest::parse(value.to_owned()).map_err(|error| ApiError::invalid(error.to_string()))
}

async fn discard(path: &Path) {
    let path = path.to_path_buf();
    match tokio::task::spawn_blocking(move || RegistryStore::discard_staging(&path)).await {
        Ok(Ok(())) => {}
        Ok(Err(error)) => eprintln!("renoa-registry: staging cleanup failed: {error}"),
        Err(error) => eprintln!("renoa-registry: failed to join staging cleanup: {error}"),
    }
}

async fn blocking<T, F>(operation: F) -> Result<T, ApiError>
where
    T: Send + 'static,
    F: FnOnce() -> Result<T, RegistryError> + Send + 'static,
{
    tokio::task::spawn_blocking(operation)
        .await
        .map_err(RegistryError::from)?
        .map_err(ApiError::from)
}

const fn default_change_limit() -> usize {
    100
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

struct ApiError {
    status: StatusCode,
    code: &'static str,
    public: String,
    internal: Option<String>,
}

impl ApiError {
    fn invalid(error: impl std::fmt::Display) -> Self {
        Self {
            status: StatusCode::BAD_REQUEST,
            code: "invalid_request",
            public: error.to_string(),
            internal: None,
        }
    }

    fn internal(error: impl std::fmt::Display) -> Self {
        Self {
            status: StatusCode::INTERNAL_SERVER_ERROR,
            code: "internal",
            public: "registry storage failed".to_owned(),
            internal: Some(error.to_string()),
        }
    }

    fn timeout() -> Self {
        Self {
            status: StatusCode::REQUEST_TIMEOUT,
            code: "timeout",
            public: "package upload exceeded five minutes".to_owned(),
            internal: None,
        }
    }
}

impl From<RegistryError> for ApiError {
    fn from(error: RegistryError) -> Self {
        match error {
            RegistryError::InvalidRequest(message) => Self::invalid(message),
            RegistryError::NotFound => Self {
                status: StatusCode::NOT_FOUND,
                code: "not_found",
                public: "package was not found".to_owned(),
                internal: None,
            },
            RegistryError::Conflict(message) => Self {
                status: StatusCode::CONFLICT,
                code: "conflict",
                public: message,
                internal: None,
            },
            other => Self::internal(other),
        }
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        if let Some(internal) = self.internal {
            eprintln!("renoa-registry: {internal}");
        }
        (
            self.status,
            Json(ErrorResponse::new(self.code, self.public)),
        )
            .into_response()
    }
}

#[cfg(test)]
mod tests {
    use super::Registry;

    #[test]
    fn registry_identity_survives_reopen() {
        let directory = tempfile::tempdir().expect("temporary registry");
        let first = Registry::open(directory.path()).expect("create registry");
        let first_id = first.store.status().expect("first status").registry_id();
        assert!(Registry::open(directory.path()).is_err());
        drop(first);

        let stale = directory
            .path()
            .join("staging")
            .join("upload-9b2aa6e3-30e6-497c-bcd9-e658324504fb.tar");
        std::fs::write(&stale, b"interrupted upload").expect("write stale upload");
        let orphan = directory.path().join("blobs").join("a".repeat(64));
        std::fs::write(&orphan, b"unacknowledged blob").expect("write orphan blob");

        let second = Registry::open(directory.path()).expect("reopen registry");
        assert!(!stale.exists());
        assert!(!orphan.exists());
        assert_eq!(
            second.store.status().expect("second status").registry_id(),
            first_id
        );
    }
}
