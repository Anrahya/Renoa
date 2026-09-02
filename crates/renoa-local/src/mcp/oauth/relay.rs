use std::{path::Path, sync::Arc, time::Duration};

use renoa_oauth_relay_protocol::{
    AcknowledgeOAuthRelayRequest, AcknowledgeOAuthRelayResponse, CreateOAuthRelayRequest,
    CreateOAuthRelayResponse, DEVICE_ID_HEADER, OAUTH_CALLBACK_PATH, OAUTH_RELAY_VERSION,
    OAUTH_RELAYS_PATH, OAuthRelayId, OAuthRelayStatus,
};
use reqwest::{Client, header};
use serde::Deserialize;
use tokio_util::sync::CancellationToken;
use url::Url;
use uuid::Uuid;

use super::{
    SensitiveString,
    relay_http::{HTTP_DEADLINE, decode_success, relay_unavailable, send_with_retry},
};
use crate::mcp::{McpHostError, McpOAuthError, hex_sha256};

const POLL_INTERVAL: Duration = Duration::from_secs(1);
const MAX_CREDENTIAL_FILE_BYTES: u64 = 16 * 1024;
const MAX_RELAY_LIFETIME: Duration = Duration::from_mins(11);

#[derive(Clone)]
pub(super) struct OAuthRelayClient {
    inner: Arc<OAuthRelayClientInner>,
}

struct OAuthRelayClientInner {
    origin: Url,
    callback_uri: String,
    credentials: RelayCredentials,
    http: Client,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct RelayCredentials {
    device_id: Uuid,
    credential: SensitiveString,
}

pub(super) struct RemoteOAuthCallback {
    client: OAuthRelayClient,
    relay_id: OAuthRelayId,
    expires_at_ms: i64,
}

pub(super) enum RemoteCallbackData {
    Authorized {
        authorization_code: String,
        issuer: Option<String>,
    },
    Rejected {
        error: String,
    },
}

impl OAuthRelayClient {
    pub(super) fn new(origin: &str, credential_file: &Path) -> Result<Self, McpHostError> {
        let origin = validate_origin(origin)?;
        let callback_uri = origin.join(OAUTH_CALLBACK_PATH).map_err(|_| {
            McpOAuthError::Invalid("OAuth relay callback origin is malformed".to_owned())
        })?;
        let credentials = read_credentials(credential_file)?;
        let http = Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .timeout(HTTP_DEADLINE)
            .build()
            .map_err(|error| relay_unavailable(&format!("HTTP client setup failed: {error}")))?;
        Ok(Self {
            inner: Arc::new(OAuthRelayClientInner {
                origin,
                callback_uri: callback_uri.to_string(),
                credentials,
                http,
            }),
        })
    }

    pub(super) fn callback_uri(&self) -> &str {
        &self.inner.callback_uri
    }

    pub(super) async fn create(
        &self,
        state: &str,
        cancellation: &CancellationToken,
    ) -> Result<RemoteOAuthCallback, McpHostError> {
        require_active(cancellation)?;
        let relay_id = OAuthRelayId::new();
        let request = CreateOAuthRelayRequest {
            version: OAUTH_RELAY_VERSION,
            relay_id,
            state_digest: hex_sha256(state.as_bytes()),
        };
        let endpoint = self.endpoint(OAUTH_RELAYS_PATH)?;
        let body = serde_json::to_vec(&request)?;
        let response = send_with_retry(
            || {
                self.authorized(self.inner.http.post(endpoint.clone()))
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(body.clone())
            },
            cancellation,
        )
        .await?;
        let response: CreateOAuthRelayResponse = decode_success(response).await?;
        if response.version != OAUTH_RELAY_VERSION
            || response.relay_id != relay_id
            || response.redirect_uri != self.inner.callback_uri
        {
            return Err(relay_unavailable(
                "relay reservation changed the requested callback identity",
            ));
        }
        validate_expiry(response.expires_at_ms)?;
        Ok(RemoteOAuthCallback {
            client: self.clone(),
            relay_id,
            expires_at_ms: response.expires_at_ms,
        })
    }

    pub(super) fn resume(
        &self,
        relay_id: OAuthRelayId,
        expires_at_ms: i64,
    ) -> Result<RemoteOAuthCallback, McpHostError> {
        if expires_at_ms <= now_ms()? {
            return Err(McpOAuthError::CallbackExpired.into());
        }
        Ok(RemoteOAuthCallback {
            client: self.clone(),
            relay_id,
            expires_at_ms,
        })
    }

    pub(super) async fn acknowledge_saved(
        &self,
        relay_id: OAuthRelayId,
        expires_at_ms: i64,
        cancellation: &CancellationToken,
    ) -> Result<(), McpHostError> {
        if expires_at_ms <= now_ms()? {
            return Ok(());
        }
        self.resume(relay_id, expires_at_ms)?
            .acknowledge(cancellation)
            .await
    }

    fn endpoint(&self, path: &str) -> Result<Url, McpHostError> {
        self.inner
            .origin
            .join(path)
            .map_err(|_| relay_unavailable("relay endpoint is malformed"))
    }

    fn authorized(&self, request: reqwest::RequestBuilder) -> reqwest::RequestBuilder {
        request
            .header(
                DEVICE_ID_HEADER,
                self.inner.credentials.device_id.to_string(),
            )
            .bearer_auth(self.inner.credentials.credential.expose())
    }
}

impl RemoteOAuthCallback {
    pub(super) const fn relay_id(&self) -> OAuthRelayId {
        self.relay_id
    }

    pub(super) const fn expires_at_ms(&self) -> i64 {
        self.expires_at_ms
    }

    pub(super) fn redirect_uri(&self) -> &str {
        self.client.callback_uri()
    }

    pub(super) async fn receive(
        &self,
        cancellation: &CancellationToken,
    ) -> Result<RemoteCallbackData, McpHostError> {
        let status_path = format!("{OAUTH_RELAYS_PATH}/{}", self.relay_id);
        loop {
            let remaining = remaining(self.expires_at_ms)?;
            require_active(cancellation)?;
            let endpoint = self.client.endpoint(&status_path)?;
            let response = send_with_retry(
                || {
                    self.client
                        .authorized(self.client.inner.http.get(endpoint.clone()))
                },
                cancellation,
            )
            .await?;
            let status: OAuthRelayStatus = decode_success(response).await?;
            if status.version() != OAUTH_RELAY_VERSION {
                return Err(relay_unavailable("relay returned an unsupported version"));
            }
            match status {
                OAuthRelayStatus::Pending { .. } => {
                    tokio::select! {
                        biased;
                        () = cancellation.cancelled() => return Err(McpOAuthError::Cancelled.into()),
                        () = tokio::time::sleep(POLL_INTERVAL.min(remaining)) => {}
                    }
                }
                OAuthRelayStatus::Authorized {
                    authorization_code,
                    issuer,
                    ..
                } => {
                    return Ok(RemoteCallbackData::Authorized {
                        authorization_code,
                        issuer,
                    });
                }
                OAuthRelayStatus::Rejected { error, .. } => {
                    return Ok(RemoteCallbackData::Rejected { error });
                }
                OAuthRelayStatus::Acknowledged { .. } => {
                    return Err(relay_unavailable(
                        "callback was acknowledged before this Host retained it",
                    ));
                }
            }
        }
    }

    pub(super) async fn acknowledge(
        &self,
        cancellation: &CancellationToken,
    ) -> Result<(), McpHostError> {
        require_active(cancellation)?;
        let path = format!("{OAUTH_RELAYS_PATH}/{}/acknowledge", self.relay_id);
        let endpoint = self.client.endpoint(&path)?;
        let request = AcknowledgeOAuthRelayRequest {
            version: OAUTH_RELAY_VERSION,
        };
        let body = serde_json::to_vec(&request)?;
        let response = send_with_retry(
            || {
                self.client
                    .authorized(self.client.inner.http.post(endpoint.clone()))
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(body.clone())
            },
            cancellation,
        )
        .await?;
        let acknowledgement: AcknowledgeOAuthRelayResponse = decode_success(response).await?;
        if acknowledgement.version != OAUTH_RELAY_VERSION {
            return Err(relay_unavailable(
                "relay acknowledgement returned an unsupported version",
            ));
        }
        Ok(())
    }
}

fn validate_origin(value: &str) -> Result<Url, McpHostError> {
    let origin = Url::parse(value)
        .map_err(|_| McpOAuthError::Invalid("OAuth relay origin is not a URL".to_owned()))?;
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
        return Err(McpOAuthError::Invalid(
            "OAuth relay must be an HTTPS origin, except for loopback tests".to_owned(),
        )
        .into());
    }
    Ok(origin)
}

fn read_credentials(path: &Path) -> Result<RelayCredentials, McpHostError> {
    if !path.is_absolute() {
        return Err(McpOAuthError::Invalid(
            "OAuth relay credential path must be absolute".to_owned(),
        )
        .into());
    }
    let metadata = std::fs::symlink_metadata(path)?;
    if !metadata.file_type().is_file()
        || metadata.file_type().is_symlink()
        || metadata.len() == 0
        || metadata.len() > MAX_CREDENTIAL_FILE_BYTES
    {
        return Err(McpOAuthError::Invalid(
            "OAuth relay credential must be a bounded regular file".to_owned(),
        )
        .into());
    }
    require_private(path, &metadata)?;
    let mut bytes = std::fs::read(path)?;
    let decoded = serde_json::from_slice::<RelayCredentials>(&bytes);
    bytes.fill(0);
    let credentials = decoded.map_err(|_| {
        McpOAuthError::Invalid("OAuth relay credential file is malformed".to_owned())
    })?;
    let credential = credentials.credential.expose();
    if credential.len() != 64
        || !credential
            .bytes()
            .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'))
    {
        return Err(
            McpOAuthError::Invalid("OAuth relay credential file is malformed".to_owned()).into(),
        );
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
    let credentials_directory =
        std::env::var_os("CREDENTIALS_DIRECTORY").map(std::path::PathBuf::from);
    if mode == 0o440
        && credentials_directory.as_deref() == path.parent()
        && credentials_directory.as_deref().is_some_and(|directory| {
            std::fs::symlink_metadata(directory).is_ok_and(|directory_metadata| {
                let directory_mode = directory_metadata.permissions().mode() & 0o777;
                directory_metadata.file_type().is_dir()
                    && !directory_metadata.file_type().is_symlink()
                    && matches!(directory_mode, 0o500 | 0o550)
                    && directory_metadata.uid() == metadata.uid()
                    && directory_metadata.gid() == metadata.gid()
            })
        })
    {
        return Ok(());
    }
    Err(McpOAuthError::Invalid(
        "OAuth relay credential must not be accessible by group or other users".to_owned(),
    )
    .into())
}

#[cfg(not(unix))]
fn require_private(_path: &Path, _metadata: &std::fs::Metadata) -> Result<(), McpHostError> {
    Ok(())
}

fn validate_expiry(expires_at_ms: i64) -> Result<(), McpHostError> {
    let now = now_ms()?;
    let max = now.saturating_add(i64::try_from(MAX_RELAY_LIFETIME.as_millis()).unwrap_or(i64::MAX));
    if expires_at_ms <= now || expires_at_ms > max {
        return Err(relay_unavailable("relay returned an invalid expiry"));
    }
    Ok(())
}

fn remaining(expires_at_ms: i64) -> Result<Duration, McpHostError> {
    u64::try_from(expires_at_ms.saturating_sub(now_ms()?))
        .ok()
        .filter(|millis| *millis > 0)
        .map(Duration::from_millis)
        .ok_or_else(|| McpOAuthError::CallbackExpired.into())
}

fn now_ms() -> Result<i64, McpHostError> {
    let elapsed = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_err(|_| McpOAuthError::Invalid("system clock is before Unix time".to_owned()))?;
    i64::try_from(elapsed.as_millis())
        .map_err(|_| McpOAuthError::Invalid("system clock exceeds i64".to_owned()).into())
}

fn require_active(cancellation: &CancellationToken) -> Result<(), McpHostError> {
    if cancellation.is_cancelled() {
        Err(McpOAuthError::Cancelled.into())
    } else {
        Ok(())
    }
}
