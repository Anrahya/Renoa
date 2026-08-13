use std::io::Write;

use renoa_agent::{AssistantContent, ContentBlock, Message, ModelRequest};

const REQUEST_FRAME_TOKENS: u64 = 64;
const MESSAGE_FRAME_TOKENS: u64 = 12;
const CONTENT_FRAME_TOKENS: u64 = 4;
const TOOL_FRAME_TOKENS: u64 = 24;
const IMAGE_TOKENS: u64 = 4_096;
const ESTIMATED_BYTES_PER_TOKEN: u64 = 3;

pub(super) fn estimate_input_tokens(request: &ModelRequest) -> u64 {
    let mut total = REQUEST_FRAME_TOKENS.saturating_add(text(&request.system_prompt));
    for message in &request.messages {
        total = total
            .saturating_add(MESSAGE_FRAME_TOKENS)
            .saturating_add(message_tokens(message));
    }
    for tool in &request.tools {
        total = total
            .saturating_add(TOOL_FRAME_TOKENS)
            .saturating_add(text(&tool.name))
            .saturating_add(text(&tool.description))
            .saturating_add(json(&tool.input_schema));
    }
    total
}

fn message_tokens(message: &Message) -> u64 {
    match message {
        Message::User { content } => content_tokens(content),
        Message::Assistant {
            content, metadata, ..
        } => {
            let mut total = 0_u64;
            for block in content {
                total = total
                    .saturating_add(CONTENT_FRAME_TOKENS)
                    .saturating_add(match block {
                        AssistantContent::Text {
                            text: value,
                            signature,
                        }
                        | AssistantContent::Reasoning {
                            text: value,
                            signature,
                            ..
                        } => text(value).saturating_add(optional_text(signature.as_deref())),
                        AssistantContent::ToolCall { call } => text(&call.id)
                            .saturating_add(text(&call.name))
                            .saturating_add(json(&call.arguments))
                            .saturating_add(optional_text(call.thought_signature.as_deref()))
                            .saturating_add(optional_text(call.namespace.as_deref())),
                    });
            }
            for value in [
                metadata.api.as_deref(),
                metadata.provider.as_deref(),
                metadata.model.as_deref(),
                metadata.response_model.as_deref(),
                metadata.response_id.as_deref(),
                metadata.raw_stop_reason.as_deref(),
            ] {
                total = total.saturating_add(optional_text(value));
            }
            total
        }
        Message::Tool { result } => text(&result.call_id)
            .saturating_add(text(&result.name))
            .saturating_add(content_tokens(&result.content))
            .saturating_add(result.details.as_ref().map_or(0, json)),
    }
}

fn content_tokens(content: &[ContentBlock]) -> u64 {
    content.iter().fold(0_u64, |total, block| {
        total
            .saturating_add(CONTENT_FRAME_TOKENS)
            .saturating_add(match block {
                ContentBlock::Text { text: value } => text(value),
                ContentBlock::Image { mime_type, .. } => {
                    IMAGE_TOKENS.saturating_add(text(mime_type))
                }
            })
    })
}

fn optional_text(value: Option<&str>) -> u64 {
    value.map_or(0, text)
}

fn json(value: &serde_json::Value) -> u64 {
    let mut counter = ByteCounter(0);
    if serde_json::to_writer(&mut counter, value).is_err() {
        return u64::MAX;
    }
    counter.0.div_ceil(ESTIMATED_BYTES_PER_TOKEN)
}

struct ByteCounter(u64);

impl Write for ByteCounter {
    fn write(&mut self, buffer: &[u8]) -> std::io::Result<usize> {
        self.0 = self
            .0
            .saturating_add(u64::try_from(buffer.len()).unwrap_or(u64::MAX));
        Ok(buffer.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

fn text(value: &str) -> u64 {
    bytes(value.len())
}

fn bytes(value: usize) -> u64 {
    u64::try_from(value)
        .unwrap_or(u64::MAX)
        .div_ceil(ESTIMATED_BYTES_PER_TOKEN)
}

#[cfg(test)]
mod tests {
    use renoa_agent::{ContentBlock, Message, ModelRequest, ToolSpec};

    use super::estimate_input_tokens;

    #[test]
    fn text_and_tool_schema_increase_the_estimate() {
        let empty = request(Vec::new(), Vec::new());
        let text = request(vec![Message::user_text("x".repeat(3_000))], Vec::new());
        let tool = request(
            vec![Message::user_text("x".repeat(3_000))],
            vec![ToolSpec {
                name: "read_file".to_owned(),
                description: "Read one file.".to_owned(),
                input_schema: serde_json::json!({
                    "type": "object",
                    "properties": { "path": { "type": "string" } }
                }),
            }],
        );

        assert!(estimate_input_tokens(&empty) > 0);
        assert!(estimate_input_tokens(&text) > estimate_input_tokens(&empty));
        assert!(estimate_input_tokens(&tool) > estimate_input_tokens(&text));
    }

    #[test]
    fn inline_image_bytes_do_not_masquerade_as_text_tokens() {
        let small = image_request("a");
        let large = image_request(&"a".repeat(1_000_000));

        assert_eq!(estimate_input_tokens(&small), estimate_input_tokens(&large));
    }

    fn request(messages: Vec<Message>, tools: Vec<ToolSpec>) -> ModelRequest {
        ModelRequest {
            system_prompt: "Be precise.".to_owned(),
            messages,
            tools,
        }
    }

    fn image_request(data: &str) -> ModelRequest {
        request(
            vec![Message::User {
                content: vec![ContentBlock::image(data, "image/png")],
            }],
            Vec::new(),
        )
    }
}
