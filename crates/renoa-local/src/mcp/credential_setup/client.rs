use std::{path::Path, sync::Arc, time::Duration};

use futures_util::StreamExt as _;
use renoa_credential_relay_protocol::{
    AcknowledgeCredentialRelayRequest, AcknowledgeCredentialRelayResponse,
    CREDENTIAL_RELAY_VERSION, CREDENTIAL_RELAYS_PATH, CreateCredentialRelayRequest,
    CreateCredentialRelayResponse, CredentialRelayErrorResponse, CredentialRelayId,
    CredentialRelayStatus, DEVICE_ID_HEADER,
};
use reqwest::{Client, header};
use serde::Deserialize;
use tokio_util::sync::CancellationToken;
use url::Url;
use uuid::Uuid;

use super::state::CredentialSetupState;
use crate::mcp::{McpCredentialError, McpHostError, oauth::SensitiveString};

const HTTP_DEADLINE: Duration = Duration::from_secs(15);
const MAX_RESPONSE_BYTES: usize = 160 * 1024;
const MAX_CREDENTIAL_FILE_BYTES: u64 = 16 * 1024;
const MAX_RELAY_LIFETIME: Duration = Duration::from_mins(16);
const MAX_HTTP_ATTEMPTS: u32 = 3;

#[derive(Clone)]
pub(super) struct CredentialRelayClient {
    inner: Arc<ClientInner>,
}

struct ClientInner {
    origin: Url,
    credentials: RelayCredentials,
    http: Client,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct RelayCredentials {
    device_id: Uuid,
    credential: SensitiveString,
}

impl CredentialRelayClient {
    pub(super) fn new(origin: &str, credential_file: &Path) -> Result<Self, McpHostError> {
        let origin = validate_origin(origin)?;
        let credentials = read_credentials(credential_file)?;
        let http = Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .timeout(HTTP_DEADLINE)
            .build()
            .map_err(|error| unavailable(&format!("HTTP client setup failed: {error}")))?;
        Ok(Self {
            inner: Arc::new(ClientInner {
                origin,
                credentials,
                http,
            }),
        })
    }

    pub(super) async fn reserve(
        &self,
        state: &CredentialSetupState,
        cancellation: &CancellationToken,
    ) -> Result<i64, McpHostError> {
        let request = CreateCredentialRelayRequest {
            version: CREDENTIAL_RELAY_VERSION,
            relay_id: state.relay_id,
            credential_id: state.credential_id.clone(),
            kind: state.kind,
            capability_digest: crate::mcp::hex_sha256(state.capability.expose().as_bytes()),
        };
        let body = serde_json::to_vec(&request)?;
        let endpoint = self.endpoint(CREDENTIAL_RELAYS_PATH)?;
        let response = self
            .send(
                || {
                    self.authorized(self.inner.http.post(endpoint.clone()))
                        .header(header::CONTENT_TYPE, "application/json")
                        .body(body.clone())
                },
                cancellation,
            )
            .await?;
        let response: CreateCredentialRelayResponse = decode(response).await?;
        if response.version != CREDENTIAL_RELAY_VERSION || response.relay_id != state.relay_id {
            return Err(unavailable("relay changed the requested identity"));
        }
        validate_expiry(response.expires_at_ms)?;
        Ok(response.expires_at_ms)
    }

    pub(super) fn setup_url(&self, state: &CredentialSetupState) -> Result<Url, McpHostError> {
        let mut url = self.endpoint(&format!(
            "{CREDENTIAL_RELAYS_PATH}/{}/setup",
            state.relay_id
        ))?;
        let mut fragment = url::form_urlencoded::Serializer::new(String::new());
        fragment
            .append_pair("v", &CREDENTIAL_RELAY_VERSION.to_string())
            .append_pair("key", state.key.expose())
            .append_pair("token", state.capability.expose());
        if let Some(issuer) = state.expected_issuer.as_deref() {
            fragment.append_pair("issuer", issuer);
        }
        url.set_fragment(Some(&fragment.finish()));
        Ok(url)
    }

    pub(super) async fn status(
        &self,
        relay_id: CredentialRelayId,
        cancellation: &CancellationToken,
    ) -> Result<CredentialRelayStatus, McpHostError> {
        let endpoint = self.endpoint(&format!("{CREDENTIAL_RELAYS_PATH}/{relay_id}"))?;
        let response = self
            .send(
                || self.authorized(self.inner.http.get(endpoint.clone())),
                cancellation,
            )
            .await?;
        let status: CredentialRelayStatus = decode(response).await?;
        if status.version() != CREDENTIAL_RELAY_VERSION {
            return Err(unavailable("relay returned an unsupported version"));
        }
        Ok(status)
    }

    pub(super) async fn acknowledge(
        &self,
        relay_id: CredentialRelayId,
        cancellation: &CancellationToken,
    ) -> Result<(), McpHostError> {
        let endpoint =
            self.endpoint(&format!("{CREDENTIAL_RELAYS_PATH}/{relay_id}/acknowledge"))?;
        let body = serde_json::to_vec(&AcknowledgeCredentialRelayRequest {
            version: CREDENTIAL_RELAY_VERSION,
        })?;
        let response = self
            .send(
                || {
                    self.authorized(self.inner.http.post(endpoint.clone()))
                        .header(header::CONTENT_TYPE, "application/json")
                        .body(body.clone())
                },
                cancellation,
            )
            .await?;
        let response: AcknowledgeCredentialRelayResponse = decode(response).await?;
        if response.version != CREDENTIAL_RELAY_VERSION {
            return Err(unavailable(
                "relay acknowledgement returned an unsupported version",
            ));
        }
        Ok(())
    }

    fn endpoint(&self, path: &str) -> Result<Url, McpHostError> {
        self.inner
            .origin
            .join(path)
            .map_err(|_| unavailable("relay endpoint is malformed"))
    }

    fn authorized(&self, request: reqwest::RequestBuilder) -> reqwest::RequestBuilder {
        request
            .header(
                DEVICE_ID_HEADER,
                self.inner.credentials.device_id.to_string(),
            )
            .bearer_auth(self.inner.credentials.credential.expose())
    }

    async fn send(
        &self,
        mut request: impl FnMut() -> reqwest::RequestBuilder,
        cancellation: &CancellationToken,
    ) -> Result<reqwest::Response, McpHostError> {
        for attempt in 1..=MAX_HTTP_ATTEMPTS {
            if cancellation.is_cancelled() {
                return Err(McpCredentialError::Cancelled.into());
            }
            let result = tokio::select! {
                biased;
                () = cancellation.cancelled() => return Err(McpCredentialError::Cancelled.into()),
                result = request().send() => result,
            };
            match result {
                Ok(response) if !retryable(response.status()) || attempt == MAX_HTTP_ATTEMPTS => {
                    return Ok(response);
                }
                Ok(_response) => {}
                Err(error) if attempt == MAX_HTTP_ATTEMPTS => {
                    return Err(unavailable(&error.to_string()));
                }
                Err(_) => {}
            }
            let delay = Duration::from_millis(200_u64.saturating_mul(u64::from(attempt)));
            tokio::select! {
                biased;
                () = cancellation.cancelled() => return Err(McpCredentialError::Cancelled.into()),
                () = tokio::time::sleep(delay) => {}
            }
        }
        Err(unavailable("relay request attempts were exhausted"))
    }
}

async fn decode<T: serde::de::DeserializeOwned>(
    response: reqwest::Response,
) -> Result<T, McpHostError> {
    let status = response.status();
    let mut body = read_bounded(response).await?;
    if !status.is_success() {
        let parsed = serde_json::from_slice::<CredentialRelayErrorResponse>(&body).ok();
        body.fill(0);
        return Err(unavailable(&parsed.map_or_else(
            || format!("HTTP {status}"),
            |error| format!("{} ({status}): {}", error.code, error.message),
        )));
    }
    let parsed =
        serde_json::from_slice(&body).map_err(|_| unavailable("relay returned malformed JSON"));
    body.fill(0);
    parsed
}

async fn read_bounded(response: reqwest::Response) -> Result<Vec<u8>, McpHostError> {
    if response
        .content_length()
        .is_some_and(|length| length > MAX_RESPONSE_BYTES as u64)
    {
        return Err(unavailable("relay response exceeded its boundary"));
    }
    let mut stream = response.bytes_stream();
    let mut body = Vec::new();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|error| unavailable(&error.to_string()))?;
        if body.len().saturating_add(chunk.len()) > MAX_RESPONSE_BYTES {
            body.fill(0);
            return Err(unavailable("relay response exceeded its boundary"));
        }
        body.extend_from_slice(&chunk);
    }
    Ok(body)
}

fn validate_origin(value: &str) -> Result<Url, McpHostError> {
    let origin = Url::parse(value).map_err(|_| unavailable("relay origin is not a URL"))?;
    let loopback = origin.scheme() == "http"
        && origin.host_str().is_some_and(|host| {
            host.eq_ignore_ascii_case("localhost")
                || host
                    .trim_matches(['[', ']'])
                    .parse::<std::net::IpAddr>()
                    .is_ok_and(|address| address.is_loopback())
        });
    if (origin.scheme() != "https" && !loopback)
        || origin.host_str().is_none()
        || !origin.username().is_empty()
        || origin.password().is_some()
        || origin.path() != "/"
        || origin.query().is_some()
        || origin.fragment().is_some()
    {
        return Err(unavailable(
            "relay must be an HTTPS origin, except for loopback tests",
        ));
    }
    Ok(origin)
}

fn read_credentials(path: &Path) -> Result<RelayCredentials, McpHostError> {
    if !path.is_absolute() {
        return Err(unavailable("relay credential path must be absolute"));
    }
    let metadata = std::fs::symlink_metadata(path)?;
    if !metadata.file_type().is_file()
        || metadata.file_type().is_symlink()
        || metadata.len() == 0
        || metadata.len() > MAX_CREDENTIAL_FILE_BYTES
    {
        return Err(unavailable(
            "relay credential must be a bounded regular file",
        ));
    }
    require_private(path, &metadata)?;
    let mut bytes = std::fs::read(path)?;
    let decoded = serde_json::from_slice::<RelayCredentials>(&bytes);
    bytes.fill(0);
    let credentials = decoded.map_err(|_| unavailable("relay credential file is malformed"))?;
    if !valid_hex(credentials.credential.expose(), 64) {
        return Err(unavailable("relay credential file is malformed"));
    }
    Ok(credentials)
}

#[cfg(unix)]
fn require_private(path: &Path, metadata: &std::fs::Metadata) -> Result<(), McpHostError> {
    use std::os::unix::fs::{MetadataExt as _, PermissionsExt as _};

    let mode = metadata.permissions().mode() & 0o777;
    if mode.trailing_zeros() >= 6 {
        return Ok(());
    }
    let directory = std::env::var_os("CREDENTIALS_DIRECTORY").map(std::path::PathBuf::from);
    if mode == 0o440
        && directory.as_deref() == path.parent()
        && directory.as_deref().is_some_and(|directory| {
            std::fs::symlink_metadata(directory).is_ok_and(|parent| {
                let parent_mode = parent.permissions().mode() & 0o777;
                parent.file_type().is_dir()
                    && !parent.file_type().is_symlink()
                    && matches!(parent_mode, 0o500 | 0o550)
                    && parent.uid() == metadata.uid()
                    && parent.gid() == metadata.gid()
            })
        })
    {
        return Ok(());
    }
    Err(unavailable(
        "relay credential must not be accessible by group or other users",
    ))
}

#[cfg(not(unix))]
fn require_private(_path: &Path, _metadata: &std::fs::Metadata) -> Result<(), McpHostError> {
    Ok(())
}

fn validate_expiry(expires_at_ms: i64) -> Result<(), McpHostError> {
    let now = now_ms()?;
    let max = now.saturating_add(i64::try_from(MAX_RELAY_LIFETIME.as_millis()).unwrap_or(i64::MAX));
    if expires_at_ms <= now || expires_at_ms > max {
        return Err(unavailable("relay returned an invalid expiry"));
    }
    Ok(())
}

pub(super) fn remaining(expires_at_ms: i64) -> Result<Duration, McpHostError> {
    u64::try_from(expires_at_ms.saturating_sub(now_ms()?))
        .ok()
        .filter(|millis| *millis > 0)
        .map(Duration::from_millis)
        .ok_or_else(|| McpCredentialError::SetupExpired.into())
}

fn now_ms() -> Result<i64, McpHostError> {
    let elapsed = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_err(|_| unavailable("system clock is before Unix time"))?;
    i64::try_from(elapsed.as_millis()).map_err(|_| unavailable("system clock exceeds i64"))
}

fn valid_hex(value: &str, length: usize) -> bool {
    value.len() == length
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'))
}

fn retryable(status: reqwest::StatusCode) -> bool {
    matches!(
        status,
        reqwest::StatusCode::REQUEST_TIMEOUT
            | reqwest::StatusCode::INTERNAL_SERVER_ERROR
            | reqwest::StatusCode::BAD_GATEWAY
            | reqwest::StatusCode::SERVICE_UNAVAILABLE
            | reqwest::StatusCode::GATEWAY_TIMEOUT
    )
}

fn unavailable(detail: &str) -> McpHostError {
    McpCredentialError::SetupUnavailable(detail.to_owned()).into()
}
