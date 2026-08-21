use serde::{Deserialize, Serialize};

use crate::{
    StopReason, TokenUsage,
    tool::{ToolCall, ToolResult},
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum MessageRole {
    User,
    Assistant,
    Tool,
}

/// Ordered content accepted in user messages and returned by tools.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ContentBlock {
    Text { text: String },
    Image { data: String, mime_type: String },
}

impl ContentBlock {
    #[must_use]
    pub fn text(text: impl Into<String>) -> Self {
        Self::Text { text: text.into() }
    }

    #[must_use]
    pub fn image(data: impl Into<String>, mime_type: impl Into<String>) -> Self {
        Self::Image {
            data: data.into(),
            mime_type: mime_type.into(),
        }
    }
}

/// One ordered block in an assistant response.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum AssistantContent {
    Text {
        text: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        signature: Option<String>,
    },
    Reasoning {
        text: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        signature: Option<String>,
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        redacted: bool,
    },
    ToolCall {
        #[serde(flatten)]
        call: ToolCall,
    },
}

impl AssistantContent {
    #[must_use]
    pub fn text(text: impl Into<String>) -> Self {
        Self::Text {
            text: text.into(),
            signature: None,
        }
    }

    #[must_use]
    pub fn signed_text(text: impl Into<String>, signature: impl Into<String>) -> Self {
        Self::Text {
            text: text.into(),
            signature: Some(signature.into()),
        }
    }

    #[must_use]
    pub fn reasoning(text: impl Into<String>, signature: Option<String>, redacted: bool) -> Self {
        Self::Reasoning {
            text: text.into(),
            signature,
            redacted,
        }
    }

    #[must_use]
    pub fn tool_call(call: ToolCall) -> Self {
        Self::ToolCall { call }
    }
}

/// Provider-neutral conversation message.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "role", rename_all = "snake_case")]
pub enum Message {
    User {
        content: Vec<ContentBlock>,
    },
    Assistant {
        content: Vec<AssistantContent>,
        stop_reason: StopReason,
        usage: Option<TokenUsage>,
        #[serde(default)]
        metadata: AssistantMetadata,
    },
    Tool {
        result: ToolResult,
    },
}

/// Provider-reported identity and continuation data for one assistant response.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct AssistantMetadata {
    pub api: Option<String>,
    pub provider: Option<String>,
    pub model: Option<String>,
    pub response_model: Option<String>,
    pub response_id: Option<String>,
    pub raw_stop_reason: Option<String>,
}

impl Message {
    #[must_use]
    pub fn user_text(text: impl Into<String>) -> Self {
        Self::User {
            content: vec![ContentBlock::text(text)],
        }
    }

    #[must_use]
    pub const fn role(&self) -> MessageRole {
        match self {
            Self::User { .. } => MessageRole::User,
            Self::Assistant { .. } => MessageRole::Assistant,
            Self::Tool { .. } => MessageRole::Tool,
        }
    }
}
