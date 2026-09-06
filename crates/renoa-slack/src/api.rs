use std::time::Duration;

use futures_util::StreamExt as _;
use serde::{Deserialize, de::DeserializeOwned};
use serde_json::{Value, json};
use url::Url;

use crate::ingress::Topic;

const RESPONSE_LIMIT: usize = 1024 * 1024;

#[derive(Debug, thiserror::Error)]
pub enum ApiError {
    #[error("Slack rate limited this request; retry after {0:?}")]
    RateLimited(Duration),
    #[error("Slack rejected the request: {0}")]
    Rejected(String),
    #[error("Slack request outcome is unknown: {0}")]
    Unknown(String),
}

pub(crate) struct SlackApi {
    client: reqwest::Client,
    origin: Url,
    bot_token: String,
    app_token: String,
}

#[derive(Deserialize)]
pub(crate) struct Identity {
    #[serde(rename = "team_id")]
    pub(crate) team: String,
    #[serde(rename = "user_id")]
    pub(crate) user: String,
    #[serde(rename = "bot_id")]
    pub(crate) bot: String,
}

#[derive(Deserialize)]
pub(crate) struct SentMessage {
    pub(crate) ts: String,
}

impl SlackApi {
    pub(crate) fn new(bot_token: String, app_token: String) -> Result<Self, ApiError> {
        Self::with_origin(
            bot_token,
            app_token,
            Url::parse("https://slack.com/api/").map_err(|e| ApiError::Rejected(e.to_string()))?,
        )
    }

    pub(crate) fn with_origin(
        bot_token: String,
        app_token: String,
        origin: Url,
    ) -> Result<Self, ApiError> {
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(30))
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .map_err(|e| ApiError::Unknown(e.without_url().to_string()))?;
        Ok(Self {
            client,
            origin,
            bot_token,
            app_token,
        })
    }

    pub(crate) async fn identity(&self) -> Result<Identity, ApiError> {
        self.call("auth.test", &self.bot_token, json!({})).await
    }

    pub(crate) async fn connection_url(&self) -> Result<String, ApiError> {
        #[derive(Deserialize)]
        struct Connection {
            url: String,
        }
        let response: Connection = self
            .call("apps.connections.open", &self.app_token, json!({}))
            .await?;
        let url = Url::parse(&response.url)
            .map_err(|_| ApiError::Rejected("invalid Socket Mode URL".to_owned()))?;
        if url.scheme() != "wss"
            || url.host_str().is_none()
            || !url.username().is_empty()
            || url.password().is_some()
        {
            return Err(ApiError::Rejected(
                "Socket Mode requires an authenticated wss endpoint".to_owned(),
            ));
        }
        Ok(response.url)
    }

    pub(crate) async fn post(&self, topic: &Topic, text: &str) -> Result<SentMessage, ApiError> {
        let mut body = json!({"channel": topic.channel, "text": text, "mrkdwn": false, "unfurl_links": false, "unfurl_media": false});
        if !topic.thread.is_empty() {
            body["thread_ts"] = json!(topic.thread);
        }
        let sent: SentMessage = self.call("chat.postMessage", &self.bot_token, body).await?;
        if !crate::ingress::valid_ts(&sent.ts) {
            return Err(ApiError::Unknown(
                "Slack returned an invalid message timestamp".to_owned(),
            ));
        }
        Ok(sent)
    }

    pub(crate) async fn update(&self, topic: &Topic, ts: &str, text: &str) -> Result<(), ApiError> {
        let _: Value = self
            .call(
                "chat.update",
                &self.bot_token,
                json!({"channel": topic.channel, "ts": ts, "text": text, "mrkdwn": false}),
            )
            .await?;
        Ok(())
    }

    async fn call<T: DeserializeOwned>(
        &self,
        method: &str,
        token: &str,
        body: Value,
    ) -> Result<T, ApiError> {
        let endpoint = self
            .origin
            .join(method)
            .map_err(|e| ApiError::Rejected(e.to_string()))?;
        let response = self
            .client
            .post(endpoint)
            .bearer_auth(token)
            .json(&body)
            .send()
            .await
            .map_err(|e| ApiError::Unknown(e.without_url().to_string()))?;
        if response.status() == reqwest::StatusCode::TOO_MANY_REQUESTS {
            let seconds = response
                .headers()
                .get("retry-after")
                .and_then(|h| h.to_str().ok())
                .and_then(|h| h.parse::<u64>().ok())
                .unwrap_or(30)
                .max(1);
            return Err(ApiError::RateLimited(Duration::from_secs(seconds)));
        }
        if !response.status().is_success() {
            return Err(ApiError::Unknown(format!("HTTP {}", response.status())));
        }
        let mut stream = response.bytes_stream();
        let mut bytes = Vec::new();
        while let Some(chunk) = stream.next().await {
            let chunk = chunk.map_err(|e| ApiError::Unknown(e.without_url().to_string()))?;
            if bytes.len().saturating_add(chunk.len()) > RESPONSE_LIMIT {
                return Err(ApiError::Unknown(
                    "Slack response exceeds the size limit".to_owned(),
                ));
            }
            bytes.extend_from_slice(&chunk);
        }
        let value: Value = serde_json::from_slice(&bytes)
            .map_err(|_| ApiError::Unknown("invalid Slack JSON response".to_owned()))?;
        if value.get("ok").and_then(Value::as_bool) != Some(true) {
            return match value.get("error").and_then(Value::as_str) {
                Some("ratelimited") => Err(ApiError::RateLimited(Duration::from_secs(30))),
                Some(code @ ("internal_error" | "fatal_error" | "service_unavailable")) => {
                    Err(ApiError::Unknown(code.to_owned()))
                }
                Some(code)
                    if code.len() <= 128
                        && code.bytes().all(|c| c.is_ascii_alphanumeric() || c == b'_') =>
                {
                    Err(ApiError::Rejected(code.to_owned()))
                }
                _ => Err(ApiError::Unknown("invalid Slack error response".to_owned())),
            };
        }
        serde_json::from_value(value)
            .map_err(|_| ApiError::Unknown("incomplete Slack response".to_owned()))
    }
}
