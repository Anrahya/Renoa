use std::time::Duration;

use serde::Serialize;

use super::{ApiError, SentMessage, TelegramApi};

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
    Blocks { blocks: Vec<RichMessageBlock<'a>> },
}

#[derive(Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum RichMessageBlock<'a> {
    Paragraph { text: &'a str },
    Buttons { buttons: [UrlButton<'a>; 1] },
}

#[derive(Serialize)]
struct UrlButton<'a> {
    text: &'a str,
    url: &'a str,
}

impl TelegramApi {
    pub(crate) async fn send_markdown(
        &self,
        chat_id: i64,
        thread_id: Option<i64>,
        text: &str,
    ) -> Result<SentMessage, ApiError> {
        self.send_rich_message(chat_id, thread_id, RichMessage::Markdown { markdown: text })
            .await
    }

    pub(crate) async fn send_plain(
        &self,
        chat_id: i64,
        thread_id: Option<i64>,
        text: &str,
    ) -> Result<SentMessage, ApiError> {
        self.send_rich_message(
            chat_id,
            thread_id,
            RichMessage::Blocks {
                blocks: vec![RichMessageBlock::Paragraph { text }],
            },
        )
        .await
    }

    pub(crate) async fn send_action(
        &self,
        chat_id: i64,
        thread_id: Option<i64>,
        text: &str,
        button: &str,
        url: &str,
    ) -> Result<SentMessage, ApiError> {
        self.send_rich_message(
            chat_id,
            thread_id,
            RichMessage::Blocks {
                blocks: vec![
                    RichMessageBlock::Paragraph { text },
                    RichMessageBlock::Buttons {
                        buttons: [UrlButton { text: button, url }],
                    },
                ],
            },
        )
        .await
    }

    async fn send_rich_message(
        &self,
        chat_id: i64,
        thread_id: Option<i64>,
        rich_message: RichMessage<'_>,
    ) -> Result<SentMessage, ApiError> {
        self.call(
            "sendRichMessage",
            &RichMessageRequest {
                chat_id,
                message_thread_id: thread_id,
                rich_message,
            },
            Duration::from_secs(30),
        )
        .await
    }
}
