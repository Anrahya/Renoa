use std::time::Duration;

use serde::Deserialize;
use serde_json::json;

use crate::DiscordError;

const API: &str = "https://discord.com/api/v10";

#[derive(Debug, thiserror::Error)]
pub(crate) enum ApiError {
    #[error("Discord rate limited the request")]
    RateLimited(Duration),
    #[error("Discord rejected the request: {0}")]
    Rejected(String),
    #[error("Discord request outcome is unknown: {0}")]
    Unknown(String),
    #[error("Discord refused the bot token")]
    Unauthorized,
}

pub(crate) struct DiscordApi {
    client: reqwest::Client,
    origin: String,
    token: String,
}

impl DiscordApi {
    pub(crate) fn new(token: String) -> Result<Self, ApiError> {
        Self::with_origin(token, API.to_owned())
    }

    pub(crate) fn with_origin(token: String, origin: String) -> Result<Self, ApiError> {
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(30))
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .map_err(|error| ApiError::Unknown(error.without_url().to_string()))?;
        Ok(Self {
            client,
            origin,
            token,
        })
    }

    pub(crate) async fn gateway_url(&self) -> Result<String, ApiError> {
        #[derive(Deserialize)]
        struct Gateway {
            url: String,
        }
        let gateway: Gateway = self.get("/gateway/bot").await?;
        Ok(gateway.url)
    }

    pub(crate) async fn create_message(
        &self,
        channel_id: &str,
        content: &str,
        reply_to: Option<&str>,
    ) -> Result<String, ApiError> {
        #[derive(Deserialize)]
        struct Created {
            id: String,
        }
        let reference = reply_to.map(|message_id| json!({ "message_id": message_id }));
        let created: Created = self
            .post(
                &format!("/channels/{channel_id}/messages"),
                &json!({
                    "content": content,
                    "message_reference": reference,
                    "allowed_mentions": { "parse": [] },
                }),
            )
            .await?;
        Ok(created.id)
    }

    async fn get<T: for<'de> Deserialize<'de>>(&self, path: &str) -> Result<T, ApiError> {
        self.send(self.client.get(format!("{}{path}", self.origin)))
            .await
    }

    async fn post<T: for<'de> Deserialize<'de>>(
        &self,
        path: &str,
        body: &serde_json::Value,
    ) -> Result<T, ApiError> {
        self.send(
            self.client
                .post(format!("{}{path}", self.origin))
                .header("content-type", "application/json")
                .body(body.to_string()),
        )
        .await
    }

    async fn send<T: for<'de> Deserialize<'de>>(
        &self,
        request: reqwest::RequestBuilder,
    ) -> Result<T, ApiError> {
        let response = request
            .header("authorization", format!("Bot {}", self.token))
            .header("user-agent", "Renoa (https://renoa.live, 0.1.0)")
            .send()
            .await
            .map_err(|error| {
                if error.is_timeout() || error.is_connect() {
                    ApiError::Unknown(error.without_url().to_string())
                } else {
                    ApiError::Rejected(error.without_url().to_string())
                }
            })?;
        let status = response.status();
        if status.as_u16() == 429 {
            let delay = response
                .headers()
                .get("retry-after")
                .and_then(|value| value.to_str().ok())
                .and_then(|value| value.parse::<f64>().ok())
                .map_or(Duration::from_secs(1), |seconds| {
                    Duration::from_secs_f64(seconds.clamp(0.0, 60.0))
                });
            return Err(ApiError::RateLimited(delay));
        }
        if status.as_u16() == 401 {
            return Err(ApiError::Unauthorized);
        }
        let bytes = response.bytes().await.map_err(|error| {
            if status.is_success() {
                ApiError::Unknown(error.without_url().to_string())
            } else {
                ApiError::Rejected(error.without_url().to_string())
            }
        })?;
        if !status.is_success() {
            return Err(ApiError::Rejected(format!("HTTP {}", status.as_u16())));
        }
        serde_json::from_slice(&bytes).map_err(|error| ApiError::Rejected(error.to_string()))
    }
}

impl From<ApiError> for DiscordError {
    fn from(error: ApiError) -> Self {
        match error {
            ApiError::Unauthorized => Self::Invalid("Discord refused the bot token".to_owned()),
            other => Self::Api(other.to_string()),
        }
    }
}
