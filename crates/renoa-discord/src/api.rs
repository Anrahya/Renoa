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
        let created: Created = self
            .post(
                &format!("/channels/{channel_id}/messages"),
                &message_body(content, reply_to),
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
            .map_err(|error| ApiError::Unknown(error.without_url().to_string()))?;
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
            if status.is_success() || status.is_server_error() {
                ApiError::Unknown(error.without_url().to_string())
            } else {
                ApiError::Rejected(error.without_url().to_string())
            }
        })?;
        if status.is_server_error() {
            return Err(ApiError::Unknown(format!("HTTP {}", status.as_u16())));
        }
        if !status.is_success() {
            return Err(ApiError::Rejected(format!("HTTP {}", status.as_u16())));
        }
        serde_json::from_slice(&bytes).map_err(|error| ApiError::Unknown(error.to_string()))
    }
}

pub(crate) fn message_body(content: &str, reply_to: Option<&str>) -> serde_json::Value {
    let mut body = json!({
        "content": content,
        "allowed_mentions": { "parse": [] },
    });
    if let Some(message_id) = reply_to {
        body["message_reference"] = json!({ "message_id": message_id });
    }
    body
}

impl From<ApiError> for DiscordError {
    fn from(error: ApiError) -> Self {
        match error {
            ApiError::Unauthorized => Self::Invalid("Discord refused the bot token".to_owned()),
            other => Self::Api(other.to_string()),
        }
    }
}

#[cfg(test)]
mod tests {
    use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};

    use super::{ApiError, DiscordApi, message_body};

    #[test]
    fn a_follow_up_page_omits_message_reference() {
        let first = message_body("page", Some("101"));
        assert_eq!(first["message_reference"]["message_id"], "101");
        let next = message_body("page", None);
        assert!(next.get("message_reference").is_none());
    }

    #[tokio::test]
    async fn a_server_error_has_an_unknown_send_outcome() {
        let api = responding("500 Internal Server Error", r#"{"message":"failed"}"#).await;
        let error = api
            .create_message("202", "hello", Some("101"))
            .await
            .expect_err("server error");
        assert!(matches!(error, ApiError::Unknown(_)), "{error:?}");
    }

    #[tokio::test]
    async fn an_unreadable_success_receipt_has_an_unknown_send_outcome() {
        let api = responding("200 OK", "not-json").await;
        let error = api
            .create_message("202", "hello", Some("101"))
            .await
            .expect_err("unreadable receipt");
        assert!(matches!(error, ApiError::Unknown(_)), "{error:?}");
    }

    async fn responding(status: &'static str, body: &'static str) -> DiscordApi {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("listener");
        let address = listener.local_addr().expect("address");
        tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.expect("accept");
            let mut request = [0_u8; 4096];
            let _ = stream.read(&mut request).await.expect("request");
            let response = format!(
                "HTTP/1.1 {status}\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
                body.len()
            );
            stream
                .write_all(response.as_bytes())
                .await
                .expect("response");
        });
        DiscordApi::with_origin("token".to_owned(), format!("http://{address}")).expect("api")
    }
}
