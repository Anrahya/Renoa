//! Sending one content-free wake through FCM HTTP v1, and the settings that
//! turn push on.
//!
//! Every request carries the same body apart from the device's token: no task,
//! principal, agent or text. The app reads what changed over RCP, so nothing
//! an agent wrote reaches Google.

use std::{path::Path, time::Duration};

use reqwest::{StatusCode, header};
use tokio_util::sync::CancellationToken;
use url::Url;

use crate::{
    ControlError, PushToken,
    google_access::{AccessError, AccessTokens, GoogleCredentials},
};

const FCM_ORIGIN: &str = "https://fcm.googleapis.com";
/// The least time between two wakes to one device. The wake that arrives
/// inside it still goes out once it has passed.
const DEFAULT_DEVICE_GAP: Duration = Duration::from_secs(10);
const REQUEST_TIMEOUT: Duration = Duration::from_secs(10);
/// One send and at most two retries of a transient failure.
const MAX_ATTEMPTS: u32 = 3;
const BACKOFF_BASE: Duration = Duration::from_secs(1);
const MAX_RETRY_DELAY: Duration = Duration::from_mins(1);
const MAX_ERROR_BODY_BYTES: usize = 64 * 1024;

/// What the coordinator needs to send wakes: a Firebase project and the
/// credentials that may send to it. Without settings push is off: tokens are
/// still registered and owed wakes are dropped.
#[derive(Debug, Clone)]
pub struct PushSettings {
    credentials: GoogleCredentials,
    project_id: String,
    fcm_origin: Url,
    device_gap: Duration,
}

impl PushSettings {
    /// Reads a Google service-account key file, as the Firebase console
    /// issues it.
    ///
    /// # Errors
    ///
    /// Returns an error when the file is not a private regular file of at
    /// most 16 KiB or does not hold a usable service-account key. The error
    /// never repeats the key.
    pub fn from_service_account_file(path: impl AsRef<Path>) -> Result<Self, ControlError> {
        let (credentials, project_id) =
            GoogleCredentials::from_service_account_file(path.as_ref())?;
        Ok(Self {
            credentials,
            project_id,
            fcm_origin: Url::parse(FCM_ORIGIN)
                .map_err(|_| ControlError::store("the FCM origin is not a URL"))?,
            device_gap: DEFAULT_DEVICE_GAP,
        })
    }

    /// Sends token requests to `token_endpoint` and FCM requests to
    /// `fcm_origin` instead of Google's, as a test's local stand-ins. Hidden
    /// from the documentation: the coordinator binary never calls it, so a
    /// deployment signs its assertion for Google's token endpoint only.
    #[doc(hidden)]
    #[must_use]
    pub fn with_google_endpoints(mut self, token_endpoint: Url, fcm_origin: Url) -> Self {
        self.credentials = self.credentials.with_token_endpoint(token_endpoint);
        self.fcm_origin = fcm_origin;
        self
    }

    /// Changes the least time between two wakes to one device from its
    /// ten-second default.
    #[must_use]
    pub fn with_device_gap(mut self, gap: Duration) -> Self {
        self.device_gap = gap;
        self
    }

    pub(crate) const fn device_gap(&self) -> Duration {
        self.device_gap
    }
}

/// What sending one wake to one device came to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum WakeOutcome {
    Delivered,
    /// FCM named the token dead; its row is deleted.
    DeadToken(&'static str),
    /// A configuration fault, such as another project's token or rejected
    /// credentials. Nothing is deleted and nothing is retried.
    Refused(&'static str),
    /// A transient failure outlasted the retries. The wake is dropped: the
    /// journal is the truth, and the app reconciles from its cursors on its
    /// next connection. A wake lost this way is not delivered later.
    GaveUp(&'static str),
    /// The coordinator is shutting down.
    Cancelled,
}

#[derive(Debug, PartialEq, Eq)]
enum Attempt {
    Done(WakeOutcome),
    Retry {
        reason: &'static str,
        after: Option<Duration>,
    },
}

/// Sends wakes to FCM for one Firebase project.
#[derive(Debug)]
pub(crate) struct FcmSender {
    tokens: AccessTokens,
    http: reqwest::Client,
    send_url: Url,
}

impl FcmSender {
    /// Builds the sender. It fails `serve` at startup rather than at the
    /// first wake.
    pub(crate) fn new(settings: &PushSettings) -> Result<Self, ControlError> {
        let http = reqwest::Client::builder()
            .timeout(REQUEST_TIMEOUT)
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .map_err(|error| ControlError::store(format!("push HTTP client failed: {error}")))?;
        let send_url = settings
            .fcm_origin
            .join(&format!(
                "/v1/projects/{}/messages:send",
                settings.project_id
            ))
            .map_err(|_| ControlError::invalid("the FCM send URL is invalid"))?;
        Ok(Self {
            tokens: AccessTokens::new(settings.credentials.clone(), http.clone()),
            http,
            send_url,
        })
    }

    /// Sends one wake to `token`, retrying a transient failure a bounded
    /// number of times with exponential backoff or the server's
    /// `Retry-After`.
    pub(crate) async fn wake(
        &self,
        token: &PushToken,
        cancelled: &CancellationToken,
    ) -> WakeOutcome {
        let mut attempt = 1;
        loop {
            let result = tokio::select! {
                () = cancelled.cancelled() => return WakeOutcome::Cancelled,
                result = self.attempt(token) => result,
            };
            let (reason, after) = match result {
                Attempt::Done(outcome) => return outcome,
                Attempt::Retry { reason, after } => (reason, after),
            };
            if attempt >= MAX_ATTEMPTS {
                return WakeOutcome::GaveUp(reason);
            }
            let delay = after
                .unwrap_or(BACKOFF_BASE * 2_u32.pow(attempt - 1))
                .min(MAX_RETRY_DELAY);
            tokio::select! {
                () = cancelled.cancelled() => return WakeOutcome::Cancelled,
                () = tokio::time::sleep(delay) => {}
            }
            attempt += 1;
        }
    }

    async fn attempt(&self, token: &PushToken) -> Attempt {
        let bearer = match self.tokens.bearer().await {
            Ok(bearer) => bearer,
            Err(AccessError::Unavailable) => {
                return Attempt::Retry {
                    reason: AccessError::Unavailable.reason(),
                    after: None,
                };
            }
            Err(error) => return Attempt::Done(WakeOutcome::Refused(error.reason())),
        };
        let Ok(response) = self
            .http
            .post(self.send_url.clone())
            .header(header::AUTHORIZATION, bearer.authorization())
            .header(header::CONTENT_TYPE, "application/json")
            .body(wake_body(token))
            .send()
            .await
        else {
            return Attempt::Retry {
                reason: "FCM is unreachable",
                after: None,
            };
        };
        let status = response.status();
        if status.is_success() {
            return Attempt::Done(WakeOutcome::Delivered);
        }
        let after = retry_after(response.headers());
        let body = read_bounded(response, MAX_ERROR_BODY_BYTES)
            .await
            .unwrap_or_default();
        if status == StatusCode::UNAUTHORIZED {
            self.tokens.forget().await;
        }
        classify(status, &body, after)
    }
}

/// The request body: a data-only, high-priority message under one collapse
/// key, so an offline phone holds one pending wake. No `ttl`, so FCM's
/// four-week default applies.
fn wake_body(token: &PushToken) -> String {
    serde_json::json!({
        "message": {
            "token": token.expose(),
            "data": { "renoa": "changed" },
            "android": { "priority": "high", "collapse_key": "changed" },
        }
    })
    .to_string()
}

/// Classifies an FCM error answer by its `FcmError` code, then its details,
/// canonical status and HTTP status. A token is called dead only when a detail
/// names it: an `FcmError` of `UNREGISTERED` or `INVALID_ARGUMENT` (the payload
/// is constant and valid, so that means the token), or a `BadRequest`
/// violation on `message.token`. A bare 400 deletes nothing.
fn classify(status: StatusCode, body: &[u8], after: Option<Duration>) -> Attempt {
    let retry = |reason| Attempt::Retry { reason, after };
    let refused = |reason| Attempt::Done(WakeOutcome::Refused(reason));
    let error = ErrorBody::parse(body);
    match error.fcm_code.as_deref() {
        Some("UNREGISTERED") => return Attempt::Done(WakeOutcome::DeadToken("UNREGISTERED")),
        Some("INVALID_ARGUMENT") => {
            return Attempt::Done(WakeOutcome::DeadToken("INVALID_ARGUMENT"));
        }
        Some("SENDER_ID_MISMATCH") => return refused("SENDER_ID_MISMATCH"),
        Some("THIRD_PARTY_AUTH_ERROR") => return refused("THIRD_PARTY_AUTH_ERROR"),
        Some("QUOTA_EXCEEDED") => return retry("QUOTA_EXCEEDED"),
        Some("UNAVAILABLE") => return retry("UNAVAILABLE"),
        Some("INTERNAL") => return retry("INTERNAL"),
        _ => {}
    }
    if error.token_violation {
        return Attempt::Done(WakeOutcome::DeadToken("INVALID_ARGUMENT"));
    }
    match error.status.as_deref() {
        Some("RESOURCE_EXHAUSTED") => return retry("QUOTA_EXCEEDED"),
        Some("UNAVAILABLE") => return retry("UNAVAILABLE"),
        Some("INTERNAL") => return retry("INTERNAL"),
        _ => {}
    }
    match status {
        StatusCode::TOO_MANY_REQUESTS => retry("QUOTA_EXCEEDED"),
        StatusCode::INTERNAL_SERVER_ERROR => retry("INTERNAL"),
        StatusCode::BAD_GATEWAY => retry("BAD_GATEWAY"),
        StatusCode::SERVICE_UNAVAILABLE => retry("UNAVAILABLE"),
        StatusCode::GATEWAY_TIMEOUT => retry("GATEWAY_TIMEOUT"),
        StatusCode::UNAUTHORIZED => refused("FCM refused the access token"),
        _ => refused("FCM gave an unexpected answer"),
    }
}

/// What an FCM error body says, as far as classifying it needs.
#[derive(Default)]
struct ErrorBody {
    /// The `errorCode` of an `FcmError` detail.
    fcm_code: Option<String>,
    /// Whether a `BadRequest` detail has a violation on `message.token`.
    token_violation: bool,
    /// The error's canonical `status`.
    status: Option<String>,
}

impl ErrorBody {
    fn parse(body: &[u8]) -> Self {
        let Ok(value) = serde_json::from_slice::<serde_json::Value>(body) else {
            return Self::default();
        };
        let Some(error) = value.get("error") else {
            return Self::default();
        };
        let details = error
            .get("details")
            .and_then(serde_json::Value::as_array)
            .map_or(&[][..], Vec::as_slice);
        let of_type = |suffix: &'static str| {
            details.iter().filter(move |detail| {
                detail
                    .get("@type")
                    .and_then(serde_json::Value::as_str)
                    .is_some_and(|kind| kind.ends_with(suffix))
            })
        };
        let text = |value: Option<&serde_json::Value>| {
            value
                .and_then(serde_json::Value::as_str)
                .map(ToOwned::to_owned)
        };
        Self {
            fcm_code: of_type("google.firebase.fcm.v1.FcmError")
                .find_map(|detail| text(detail.get("errorCode"))),
            token_violation: of_type("google.rpc.BadRequest").any(|detail| {
                detail
                    .get("fieldViolations")
                    .and_then(serde_json::Value::as_array)
                    .into_iter()
                    .flatten()
                    .any(|violation| {
                        violation.get("field").and_then(serde_json::Value::as_str)
                            == Some("message.token")
                    })
            }),
            status: text(error.get("status")),
        }
    }
}

/// A `Retry-After` given in seconds.
fn retry_after(headers: &header::HeaderMap) -> Option<Duration> {
    headers
        .get(header::RETRY_AFTER)?
        .to_str()
        .ok()?
        .trim()
        .parse::<u64>()
        .ok()
        .map(Duration::from_secs)
}

/// Reads at most `limit` bytes of a response body; `None` when it fails or is
/// longer.
pub(crate) async fn read_bounded(mut response: reqwest::Response, limit: usize) -> Option<Vec<u8>> {
    let mut body = Vec::new();
    while let Some(chunk) = response.chunk().await.ok()? {
        if body.len() + chunk.len() > limit {
            return None;
        }
        body.extend_from_slice(&chunk);
    }
    Some(body)
}

#[cfg(test)]
mod tests;
