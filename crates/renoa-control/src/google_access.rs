//! How the coordinator obtains a Google OAuth access token for FCM.
//!
//! This module is the only owner of that question, and it enforces one
//! invariant: key material never leaves it. A service-account private key is
//! parsed here, held in a private type, and used only to sign the assertion
//! the token endpoint exchanges for an access token. Every other module holds
//! an opaque [`GoogleCredentials`] or [`AccessTokens`] and receives only
//! short-lived [`AccessToken`]s; each of the three redacts its `Debug`. A
//! later keyless source, such as workload identity federation with an
//! assertion the Host signs, or a broker of short-lived credentials, becomes
//! another `CredentialSource` variant here and changes nothing in the
//! dispatcher. `tests::no_other_module_handles_key_material` checks the
//! invariant against the crate's sources.

use std::{
    fmt,
    path::{Path, PathBuf},
    sync::Arc,
    time::{Duration, SystemTime},
};

use base64::{
    Engine as _,
    engine::general_purpose::{STANDARD, URL_SAFE_NO_PAD},
};
use ring::{
    rand::SystemRandom,
    signature::{RSA_PKCS1_SHA256, RsaKeyPair},
};
use serde::Deserialize;
use tokio::{sync::Mutex, time::Instant};
use url::Url;

use crate::ControlError;

const GOOGLE_TOKEN_ENDPOINT: &str = "https://oauth2.googleapis.com/token";
const FIREBASE_MESSAGING_SCOPE: &str = "https://www.googleapis.com/auth/firebase.messaging";
const JWT_BEARER_GRANT: &str = "urn:ietf:params:oauth:grant-type:jwt-bearer";
const MAX_SERVICE_ACCOUNT_FILE_BYTES: u64 = 16 * 1024;
/// Google accepts an assertion that lives at most one hour.
const ASSERTION_LIFETIME: Duration = Duration::from_hours(1);
/// A cached token is replaced this long before Google says it expires.
const REFRESH_MARGIN: Duration = Duration::from_mins(5);
/// Google issues tokens for an hour; a longer `expires_in` is not believed.
const MAX_TOKEN_LIFETIME: Duration = Duration::from_hours(1);
/// After the token endpoint rejects the credentials, requests are refused
/// this long without asking it again, rather than once per device per wake.
const REJECTION_HOLD: Duration = Duration::from_mins(1);
/// After the token endpoint fails on its side, requests fail this long
/// without asking it again, so a fast-failing endpoint is asked at most once
/// a second across every send. It is no longer than a send's first retry
/// delay, so that retry asks again.
const UNAVAILABLE_HOLD: Duration = Duration::from_secs(1);
const MAX_TOKEN_RESPONSE_BYTES: usize = 64 * 1024;

/// Credentials that obtain Google access tokens, without exposing them.
#[derive(Clone)]
pub(crate) struct GoogleCredentials {
    source: Arc<CredentialSource>,
    token_endpoint: Url,
}

/// Where access tokens come from. Each variant keeps its own material.
enum CredentialSource {
    /// A service-account key file, signed into with an RS256 assertion.
    ServiceAccount(ServiceAccountKey),
}

struct ServiceAccountKey {
    client_email: String,
    private_key_id: String,
    key: RsaKeyPair,
}

impl fmt::Debug for GoogleCredentials {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let CredentialSource::ServiceAccount(account) = &*self.source;
        formatter
            .debug_struct("GoogleCredentials")
            .field("client_email", &account.client_email)
            .field("private_key", &"[REDACTED]")
            .field("token_endpoint", &self.token_endpoint.as_str())
            .finish()
    }
}

#[derive(Deserialize)]
struct ServiceAccountFile {
    #[serde(rename = "type")]
    kind: String,
    project_id: String,
    private_key_id: String,
    private_key: String,
    client_email: String,
}

impl GoogleCredentials {
    /// Loads a service-account key file and returns its credentials and the
    /// Firebase project it belongs to. The file must be a regular file of at
    /// most 16 KiB that neither group nor other users can read, unless the
    /// service manager supplied it in `CREDENTIALS_DIRECTORY`. No error
    /// repeats the file's contents.
    pub(crate) fn from_service_account_file(path: &Path) -> Result<(Self, String), ControlError> {
        let shown = path.display();
        let metadata = std::fs::symlink_metadata(path)
            .map_err(|error| ControlError::invalid(format!("{shown} cannot be read: {error}")))?;
        if !metadata.file_type().is_file() {
            return Err(ControlError::invalid(format!(
                "{shown} must name a regular file"
            )));
        }
        if metadata.len() == 0 || metadata.len() > MAX_SERVICE_ACCOUNT_FILE_BYTES {
            return Err(ControlError::invalid(format!(
                "{shown} must hold at most {MAX_SERVICE_ACCOUNT_FILE_BYTES} bytes"
            )));
        }
        let credentials_directory = std::env::var_os("CREDENTIALS_DIRECTORY").map(PathBuf::from);
        if !renoa_home::credential_file_is_private(
            path,
            &metadata,
            credentials_directory.as_deref(),
        ) {
            return Err(ControlError::invalid(format!(
                "{shown} must not be accessible by group or other users"
            )));
        }
        let json = std::fs::read_to_string(path)
            .map_err(|error| ControlError::invalid(format!("{shown} cannot be read: {error}")))?;
        Self::from_service_account_json(&json)
            .map_err(|error| ControlError::invalid(format!("{shown}: {error}")))
    }

    fn from_service_account_json(json: &str) -> Result<(Self, String), ControlError> {
        // Every field is a string, so a serde error names a missing field or
        // the JSON position and never quotes a value.
        let file: ServiceAccountFile = serde_json::from_str(json).map_err(|error| {
            ControlError::invalid(format!("is not a Google service-account key file: {error}"))
        })?;
        if file.kind != "service_account" {
            return Err(ControlError::invalid(
                "must hold a service-account key, whose type is \"service_account\"",
            ));
        }
        let project_id_is_valid = (1..=64).contains(&file.project_id.len())
            && file
                .project_id
                .bytes()
                .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-');
        if !project_id_is_valid {
            return Err(ControlError::invalid(
                "project_id must be a Firebase project id of lowercase letters, digits and hyphens",
            ));
        }
        for (field, value) in [
            ("client_email", &file.client_email),
            ("private_key_id", &file.private_key_id),
        ] {
            if value.is_empty() || !value.bytes().all(|byte| byte.is_ascii_graphic()) {
                return Err(ControlError::invalid(format!(
                    "{field} must be non-empty printable ASCII"
                )));
            }
        }
        let key = RsaKeyPair::from_pkcs8(&pkcs8_der(&file.private_key)?).map_err(|_| {
            ControlError::invalid("private_key is not an RSA key of at least 2048 bits")
        })?;
        let credentials = Self {
            source: Arc::new(CredentialSource::ServiceAccount(ServiceAccountKey {
                client_email: file.client_email,
                private_key_id: file.private_key_id,
                key,
            })),
            token_endpoint: Url::parse(GOOGLE_TOKEN_ENDPOINT)
                .map_err(|_| ControlError::store("the Google token endpoint is not a URL"))?,
        };
        Ok((credentials, file.project_id))
    }

    /// Sends token requests to `endpoint` instead of Google's.
    pub(crate) fn with_token_endpoint(mut self, endpoint: Url) -> Self {
        self.token_endpoint = endpoint;
        self
    }
}

/// The DER body of a PEM `PRIVATE KEY` (PKCS#8) block.
fn pkcs8_der(pem: &str) -> Result<Vec<u8>, ControlError> {
    let body = pem
        .trim()
        .strip_prefix("-----BEGIN PRIVATE KEY-----")
        .and_then(|rest| rest.strip_suffix("-----END PRIVATE KEY-----"))
        .ok_or_else(|| ControlError::invalid("private_key is not a PEM PRIVATE KEY block"))?;
    let base64: String = body.split_whitespace().collect();
    STANDARD
        .decode(base64)
        .map_err(|_| ControlError::invalid("private_key is not valid base64"))
}

impl CredentialSource {
    /// The signed RS256 assertion that the token endpoint `audience`
    /// exchanges for an access token with the FCM scope.
    fn assertion(&self, audience: &str, now: SystemTime) -> Result<String, AccessError> {
        let Self::ServiceAccount(account) = self;
        let issued_at = now
            .duration_since(SystemTime::UNIX_EPOCH)
            .map_err(|_| AccessError::Signing)?
            .as_secs();
        let header = serde_json::json!({
            "alg": "RS256",
            "typ": "JWT",
            "kid": account.private_key_id,
        });
        let claims = serde_json::json!({
            "iss": account.client_email,
            "scope": FIREBASE_MESSAGING_SCOPE,
            "aud": audience,
            "iat": issued_at,
            "exp": issued_at
                .checked_add(ASSERTION_LIFETIME.as_secs())
                .ok_or(AccessError::Signing)?,
        });
        let signed = format!(
            "{}.{}",
            URL_SAFE_NO_PAD.encode(header.to_string()),
            URL_SAFE_NO_PAD.encode(claims.to_string())
        );
        let mut signature = vec![0; account.key.public().modulus_len()];
        account
            .key
            .sign(
                &RSA_PKCS1_SHA256,
                &SystemRandom::new(),
                signed.as_bytes(),
                &mut signature,
            )
            .map_err(|_| AccessError::Signing)?;
        Ok(format!("{signed}.{}", URL_SAFE_NO_PAD.encode(signature)))
    }
}

/// A short-lived bearer token for FCM.
#[derive(Clone)]
pub(crate) struct AccessToken(String);

impl AccessToken {
    pub(crate) fn authorization(&self) -> String {
        format!("Bearer {}", self.0)
    }
}

impl fmt::Debug for AccessToken {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("AccessToken([REDACTED])")
    }
}

/// Why no access token was obtained. Each reason is safe to log.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum AccessError {
    /// The token endpoint refused the credentials: a configuration fault.
    Rejected,
    /// The token endpoint could not be reached or failed on its side.
    Unavailable,
    /// The token endpoint answered with something that is not a token.
    Malformed,
    /// The assertion could not be signed.
    Signing,
}

impl AccessError {
    pub(crate) const fn reason(self) -> &'static str {
        match self {
            Self::Rejected => "the token endpoint rejected the service account",
            Self::Unavailable => "the token endpoint is unavailable",
            Self::Malformed => "the token endpoint sent no usable token",
            Self::Signing => "the token assertion could not be signed",
        }
    }

    /// How long the failure is repeated without asking the endpoint again.
    const fn hold(self) -> Duration {
        match self {
            Self::Rejected => REJECTION_HOLD,
            Self::Unavailable => UNAVAILABLE_HOLD,
            Self::Malformed | Self::Signing => Duration::ZERO,
        }
    }
}

struct CachedToken {
    token: AccessToken,
    refresh_at: Instant,
}

#[derive(Default)]
struct Cache {
    token: Option<CachedToken>,
    /// The last failed refresh and when it ended. It is repeated to every
    /// caller that waited on it, and to later ones for its error's hold.
    failed: Option<(Instant, AccessError)>,
}

/// Access tokens from one credential source, cached until shortly before they
/// expire. The cache lock is held across a refresh, so at most one refresh is
/// in flight and every caller waiting on it shares its result, a failure too:
/// a stalled token endpoint costs one timeout, not one per waiting send. A
/// rejection is then repeated for [`REJECTION_HOLD`] and an unavailable
/// endpoint for [`UNAVAILABLE_HOLD`].
pub(crate) struct AccessTokens {
    credentials: GoogleCredentials,
    http: reqwest::Client,
    cache: Mutex<Cache>,
}

impl fmt::Debug for AccessTokens {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("AccessTokens")
            .field("credentials", &self.credentials)
            .finish_non_exhaustive()
    }
}

#[derive(Deserialize)]
struct TokenResponse {
    access_token: String,
    expires_in: u64,
}

impl AccessTokens {
    pub(crate) fn new(credentials: GoogleCredentials, http: reqwest::Client) -> Self {
        Self {
            credentials,
            http,
            cache: Mutex::new(Cache::default()),
        }
    }

    pub(crate) async fn bearer(&self) -> Result<AccessToken, AccessError> {
        let asked = Instant::now();
        let mut cache = self.cache.lock().await;
        let now = Instant::now();
        if let Some(current) = &cache.token
            && now < current.refresh_at
        {
            return Ok(current.token.clone());
        }
        // A refresh that failed while this caller waited for the lock, or
        // one still inside its hold.
        if let Some((ended, error)) = cache.failed
            && (ended >= asked
                || ended
                    .checked_add(error.hold())
                    .is_some_and(|until| now < until))
        {
            return Err(error);
        }
        match self.fetch().await {
            Ok(fetched) => {
                let token = fetched.token.clone();
                *cache = Cache {
                    token: Some(fetched),
                    ..Cache::default()
                };
                Ok(token)
            }
            Err(error) => {
                cache.failed = Some((Instant::now(), error));
                Err(error)
            }
        }
    }

    /// Drops the cached token after FCM refused `failed`, unless a newer
    /// token has replaced it since.
    pub(crate) async fn forget(&self, failed: &AccessToken) {
        let mut cache = self.cache.lock().await;
        if cache
            .token
            .as_ref()
            .is_some_and(|cached| cached.token.0 == failed.0)
        {
            cache.token = None;
        }
    }

    async fn fetch(&self) -> Result<CachedToken, AccessError> {
        let source = Arc::clone(&self.credentials.source);
        let audience = self.credentials.token_endpoint.to_string();
        // RSA signing is CPU work, kept off the executor's threads.
        let assertion =
            tokio::task::spawn_blocking(move || source.assertion(&audience, SystemTime::now()))
                .await
                .map_err(|_| AccessError::Signing)??;
        let body = url::form_urlencoded::Serializer::new(String::new())
            .append_pair("grant_type", JWT_BEARER_GRANT)
            .append_pair("assertion", &assertion)
            .finish();
        let response = self
            .http
            .post(self.credentials.token_endpoint.clone())
            .header(
                reqwest::header::CONTENT_TYPE,
                "application/x-www-form-urlencoded",
            )
            .body(body)
            .send()
            .await
            .map_err(|_| AccessError::Unavailable)?;
        let status = response.status();
        // A busy or slow endpoint is not a verdict on the credentials.
        if status.is_server_error()
            || status == reqwest::StatusCode::TOO_MANY_REQUESTS
            || status == reqwest::StatusCode::REQUEST_TIMEOUT
        {
            return Err(AccessError::Unavailable);
        }
        if !status.is_success() {
            return Err(AccessError::Rejected);
        }
        let body = crate::fcm::read_bounded(response, MAX_TOKEN_RESPONSE_BYTES)
            .await
            .ok_or(AccessError::Unavailable)?;
        let answer: TokenResponse =
            serde_json::from_slice(&body).map_err(|_| AccessError::Malformed)?;
        if answer.access_token.is_empty()
            || !answer
                .access_token
                .bytes()
                .all(|byte| byte.is_ascii_graphic())
            || answer.expires_in == 0
        {
            return Err(AccessError::Malformed);
        }
        let lifetime = Duration::from_secs(answer.expires_in).min(MAX_TOKEN_LIFETIME);
        let usable = lifetime.checked_sub(REFRESH_MARGIN).unwrap_or(lifetime / 2);
        Ok(CachedToken {
            token: AccessToken(answer.access_token),
            refresh_at: Instant::now()
                .checked_add(usable)
                .ok_or(AccessError::Malformed)?,
        })
    }
}

#[cfg(test)]
pub(crate) mod tests;
