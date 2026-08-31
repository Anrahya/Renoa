use std::time::Duration;

use futures_util::StreamExt as _;
use reqwest::{Client, redirect::Policy};
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use serde_json::Value;
use thiserror::Error;

const API_ORIGIN: &str = "https://api.telegram.org";
const MAX_RESPONSE_BYTES: usize = 8 * 1024 * 1024;
const LONG_POLL_SECONDS: u32 = 30;

pub(crate) struct TelegramApi {
    client: Client,
    endpoint: String,
    token: String,
}

#[derive(Debug, Error)]
#[non_exhaustive]
pub enum ApiError {
    #[error("Telegram {method} request failed before a valid response ({category}): {detail}")]
    Transport {
        method: &'static str,
        category: &'static str,
        detail: String,
    },
    #[error("Telegram {method} response exceeded the safe size limit")]
    ResponseTooLarge { method: &'static str },
    #[error("Telegram {method} returned an invalid response")]
    InvalidResponse { method: &'static str },
    #[error("Telegram {method} rejected the request ({code}): {description}")]
    Remote {
        method: &'static str,
        code: i64,
        description: String,
        retry_after: Option<u64>,
    },
}

impl ApiError {
    pub(crate) const fn retry_after(&self) -> Option<u64> {
        match self {
            Self::Remote {
                code: 429,
                retry_after,
                ..
            } => *retry_after,
            _ => None,
        }
    }

    pub(crate) const fn is_definite_rejection(&self) -> bool {
        matches!(self, Self::Remote { .. })
    }

    pub(crate) const fn is_invalid_rich_text(&self) -> bool {
        matches!(self, Self::Remote { code: 400, .. })
    }

    pub(crate) const fn is_fatal_polling_error(&self) -> bool {
        matches!(self, Self::Remote { code, .. } if *code >= 400 && *code < 500 && *code != 429)
    }
}

#[derive(Clone, Debug, Deserialize)]
pub(crate) struct Update {
    #[serde(rename = "update_id")]
    pub(crate) id: i64,
    pub(crate) message: Option<Message>,
    pub(crate) stopped_message_generation: Option<MessageGenerationStopped>,
}

#[derive(Clone, Debug, Deserialize)]
pub(crate) struct Message {
    #[serde(rename = "message_id")]
    pub(crate) id: i64,
    #[serde(rename = "message_thread_id")]
    pub(crate) thread_id: Option<i64>,
    #[serde(rename = "from")]
    pub(crate) sender: Option<User>,
    pub(crate) chat: Chat,
    pub(crate) text: Option<String>,
}

#[derive(Clone, Debug, Deserialize)]
pub(crate) struct User {
    pub(crate) id: i64,
    pub(crate) is_bot: bool,
}

#[derive(Clone, Debug, Deserialize)]
pub(crate) struct Chat {
    pub(crate) id: i64,
    #[serde(rename = "type")]
    pub(crate) kind: String,
}

#[derive(Clone, Debug, Deserialize)]
pub(crate) struct MessageGenerationStopped {
    pub(crate) chat: Chat,
    pub(crate) message_thread_id: Option<i64>,
    pub(crate) draft_id: i64,
}

pub(crate) struct ReceivedUpdate {
    pub(crate) canonical: Vec<u8>,
    pub(crate) update: Update,
}

#[derive(Debug, Deserialize)]
pub(crate) struct BotUser {
    pub(crate) id: i64,
    pub(crate) is_bot: bool,
    pub(crate) username: Option<String>,
}

#[derive(Deserialize)]
struct WebhookInfo {
    url: String,
}

#[derive(Debug, Deserialize)]
pub(crate) struct SentMessage {
    pub(crate) message_id: i64,
}

#[derive(Deserialize)]
struct ApiResponse {
    ok: bool,
    result: Option<Value>,
    error_code: Option<i64>,
    description: Option<String>,
    parameters: Option<ResponseParameters>,
}

#[derive(Deserialize)]
struct ResponseParameters {
    retry_after: Option<u64>,
}

#[derive(Serialize)]
struct GetUpdates {
    offset: i64,
    limit: u8,
    timeout: u32,
    allowed_updates: [&'static str; 2],
}

#[derive(Serialize)]
struct Empty {}

#[derive(Serialize)]
struct SetCommands<'a> {
    commands: &'a [BotCommand],
}

#[derive(Serialize)]
struct BotCommand {
    command: &'static str,
    description: &'static str,
}

#[derive(Serialize)]
struct RichDraftRequest<'a> {
    chat_id: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    message_thread_id: Option<i64>,
    draft_id: i64,
    rich_message: RichDraftMessage<'a>,
    can_stop: bool,
    keep_on_stop: bool,
}

#[derive(Serialize)]
struct RichDraftMessage<'a> {
    blocks: Vec<RichDraftBlock<'a>>,
}

#[derive(Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum RichDraftBlock<'a> {
    Thinking { text: &'a str },
    Paragraph { text: &'a str },
}

#[derive(Serialize)]
struct RichMessageRequest<'a> {
    chat_id: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    message_thread_id: Option<i64>,
    rich_message: RichMessage<'a>,
}

#[derive(Serialize)]
#[serde(untagged)]
enum RichMessage<'a> {
    Markdown { markdown: &'a str },
    Plain { blocks: [Paragraph<'a>; 1] },
}

#[derive(Serialize)]
struct Paragraph<'a> {
    #[serde(rename = "type")]
    kind: &'static str,
    text: &'a str,
}

impl TelegramApi {
    pub(crate) fn new(token: &str) -> Result<Self, ApiError> {
        Self::with_origin(API_ORIGIN, token, true)
    }

    fn with_origin(origin: &str, token: &str, https_only: bool) -> Result<Self, ApiError> {
        let client = Client::builder()
            .https_only(https_only)
            .redirect(Policy::none())
            .no_proxy()
            .connect_timeout(Duration::from_secs(10))
            .build()
            .map_err(|error| transport_error("client initialization", error, token))?;
        Ok(Self {
            client,
            endpoint: format!("{origin}/bot{token}"),
            token: token.to_owned(),
        })
    }

    #[cfg(test)]
    pub(crate) fn for_test(origin: &str, token: &str) -> Result<Self, ApiError> {
        Self::with_origin(origin, token, false)
    }

    pub(crate) async fn get_me(&self) -> Result<BotUser, ApiError> {
        self.call("getMe", &Empty {}, Duration::from_secs(20)).await
    }

    pub(crate) async fn require_long_polling(&self) -> Result<(), ApiError> {
        let info: WebhookInfo = self
            .call("getWebhookInfo", &Empty {}, Duration::from_secs(20))
            .await?;
        if info.url.is_empty() {
            Ok(())
        } else {
            Err(ApiError::Remote {
                method: "getWebhookInfo",
                code: 409,
                description: "a webhook is configured; remove it explicitly before starting Arcee"
                    .to_owned(),
                retry_after: None,
            })
        }
    }

    pub(crate) async fn set_commands(&self) -> Result<(), ApiError> {
        let commands = [
            BotCommand {
                command: "new",
                description: "Start a fresh Arcee session in this topic",
            },
            BotCommand {
                command: "compact",
                description: "Compact this session's context",
            },
            BotCommand {
                command: "status",
                description: "Show this session's model and context use",
            },
            BotCommand {
                command: "cancel",
                description: "Stop the active turn in this topic",
            },
        ];
        let accepted: bool = self
            .call(
                "setMyCommands",
                &SetCommands {
                    commands: &commands,
                },
                Duration::from_secs(20),
            )
            .await?;
        accepted.then_some(()).ok_or(ApiError::InvalidResponse {
            method: "setMyCommands",
        })
    }

    pub(crate) async fn updates(&self, offset: i64) -> Result<Vec<ReceivedUpdate>, ApiError> {
        let values: Vec<Value> = self
            .call(
                "getUpdates",
                &GetUpdates {
                    offset,
                    limit: 100,
                    timeout: LONG_POLL_SECONDS,
                    allowed_updates: ["message", "stopped_message_generation"],
                },
                Duration::from_secs(u64::from(LONG_POLL_SECONDS) + 15),
            )
            .await?;
        let mut updates = values
            .into_iter()
            .map(|value| {
                let canonical =
                    serde_json::to_vec(&value).map_err(|_| ApiError::InvalidResponse {
                        method: "getUpdates",
                    })?;
                let update =
                    serde_json::from_value(value).map_err(|_| ApiError::InvalidResponse {
                        method: "getUpdates",
                    })?;
                Ok(ReceivedUpdate { canonical, update })
            })
            .collect::<Result<Vec<_>, _>>()?;
        updates.sort_by_key(|received| received.update.id);
        Ok(updates)
    }

    pub(crate) async fn send_draft(
        &self,
        chat_id: i64,
        thread_id: Option<i64>,
        draft_id: i64,
        thinking: Option<&str>,
        text: Option<&str>,
    ) -> Result<(), ApiError> {
        let mut blocks = Vec::with_capacity(2);
        if let Some(thinking) = thinking.filter(|value| !value.is_empty()) {
            blocks.push(RichDraftBlock::Thinking { text: thinking });
        }
        if let Some(text) = text.filter(|value| !value.is_empty()) {
            blocks.push(RichDraftBlock::Paragraph { text });
        }
        if blocks.is_empty() {
            blocks.push(RichDraftBlock::Thinking {
                text: "Thinking…"
            });
        }
        let accepted: bool = self
            .call(
                "sendRichMessageDraft",
                &RichDraftRequest {
                    chat_id,
                    message_thread_id: thread_id,
                    draft_id,
                    rich_message: RichDraftMessage { blocks },
                    can_stop: true,
                    keep_on_stop: true,
                },
                Duration::from_secs(20),
            )
            .await?;
        accepted.then_some(()).ok_or(ApiError::InvalidResponse {
            method: "sendRichMessageDraft",
        })
    }

    pub(crate) async fn send_markdown(
        &self,
        chat_id: i64,
        thread_id: Option<i64>,
        text: &str,
    ) -> Result<SentMessage, ApiError> {
        self.call(
            "sendRichMessage",
            &RichMessageRequest {
                chat_id,
                message_thread_id: thread_id,
                rich_message: RichMessage::Markdown { markdown: text },
            },
            Duration::from_secs(30),
        )
        .await
    }

    pub(crate) async fn send_plain(
        &self,
        chat_id: i64,
        thread_id: Option<i64>,
        text: &str,
    ) -> Result<SentMessage, ApiError> {
        self.call(
            "sendRichMessage",
            &RichMessageRequest {
                chat_id,
                message_thread_id: thread_id,
                rich_message: RichMessage::Plain {
                    blocks: [Paragraph {
                        kind: "paragraph",
                        text,
                    }],
                },
            },
            Duration::from_secs(30),
        )
        .await
    }

    async fn call<T: DeserializeOwned>(
        &self,
        method: &'static str,
        payload: &impl Serialize,
        timeout: Duration,
    ) -> Result<T, ApiError> {
        let body = serde_json::to_vec(payload).map_err(|_| ApiError::InvalidResponse { method })?;
        let response = self
            .client
            .post(format!("{}/{method}", self.endpoint))
            .header(reqwest::header::CONTENT_TYPE, "application/json")
            .timeout(timeout)
            .body(body)
            .send()
            .await
            .map_err(|error| transport_error(method, error, &self.token))?;
        let bytes = bounded_body(response, method, &self.token).await?;
        let envelope = serde_json::from_slice::<ApiResponse>(&bytes)
            .map_err(|_| ApiError::InvalidResponse { method })?;
        if !envelope.ok {
            return Err(ApiError::Remote {
                method,
                code: envelope.error_code.unwrap_or_default(),
                description: safe_description(envelope.description.as_deref(), &self.token),
                retry_after: envelope.parameters.and_then(|value| value.retry_after),
            });
        }
        serde_json::from_value(
            envelope
                .result
                .ok_or(ApiError::InvalidResponse { method })?,
        )
        .map_err(|_| ApiError::InvalidResponse { method })
    }
}

async fn bounded_body(
    response: reqwest::Response,
    method: &'static str,
    token: &str,
) -> Result<Vec<u8>, ApiError> {
    let mut stream = response.bytes_stream();
    let mut body = Vec::new();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|error| transport_error(method, error, token))?;
        if body.len().saturating_add(chunk.len()) > MAX_RESPONSE_BYTES {
            return Err(ApiError::ResponseTooLarge { method });
        }
        body.extend_from_slice(&chunk);
    }
    Ok(body)
}

fn transport_error(method: &'static str, error: reqwest::Error, token: &str) -> ApiError {
    let category = transport_category(&error);
    let detail = safe_text(&error.without_url().to_string(), token);
    ApiError::Transport {
        method,
        category,
        detail,
    }
}

fn transport_category(error: &reqwest::Error) -> &'static str {
    if error.is_timeout() {
        "timeout"
    } else if error.is_connect() {
        "connection"
    } else if error.is_body() || error.is_decode() {
        "response"
    } else {
        "transport"
    }
}

fn safe_description(description: Option<&str>, token: &str) -> String {
    safe_text(description.unwrap_or("request rejected"), token)
}

fn safe_text(text: &str, token: &str) -> String {
    let redacted = if token.is_empty() {
        text.to_owned()
    } else {
        text.replace(token, "[REDACTED]")
    };
    redacted
        .chars()
        .filter(|character| !character.is_control() || *character == '\n')
        .take(1024)
        .collect()
}

#[cfg(test)]
mod tests;
