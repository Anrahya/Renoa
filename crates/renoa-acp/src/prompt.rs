use std::sync::Arc;

use agent_client_protocol::schema::v1::{ContentBlock as AcpContentBlock, Meta};
use renoa_agent::{AgentEventSink, BoxFuture, ContentBlock};
use renoa_local::{AgentSession, LocalTurnOutcome};
use uuid::Uuid;

use crate::ServerError;

pub(crate) enum PromptAction {
    Turn(Vec<ContentBlock>),
    Compact,
}

pub(crate) async fn execute(
    session: &Arc<AgentSession>,
    action: PromptAction,
    request_id: Uuid,
    sink: Arc<dyn AgentEventSink>,
) -> Result<LocalTurnOutcome, ServerError> {
    Ok(match action {
        PromptAction::Turn(content) => session.execute_turn(request_id, content, sink).await?,
        PromptAction::Compact => session.execute_compaction(request_id, sink).await?,
    })
}

pub(crate) fn action(blocks: Vec<AcpContentBlock>) -> Result<PromptAction, ServerError> {
    if blocks.is_empty() {
        return Err(ServerError::InvalidRequest(
            "prompt must contain at least one content block".to_owned(),
        ));
    }
    if blocks.len() == 1
        && matches!(&blocks[0], AcpContentBlock::Text(text) if text.text.trim() == "/compact")
    {
        return Ok(PromptAction::Compact);
    }
    if blocks.iter().any(is_compact_invocation) {
        return Err(ServerError::InvalidRequest(
            "/compact does not accept arguments or attachments".to_owned(),
        ));
    }
    blocks
        .into_iter()
        .map(content_block)
        .collect::<Result<Vec<_>, _>>()
        .map(PromptAction::Turn)
}

pub(crate) fn silent_sink() -> Arc<dyn AgentEventSink> {
    Arc::new(SilentSink)
}

struct SilentSink;

impl AgentEventSink for SilentSink {
    fn emit(&self, _event: renoa_agent::AgentEvent) -> BoxFuture<'_, ()> {
        Box::pin(async {})
    }
}

pub(crate) fn request_identity(meta: Option<&Meta>) -> Result<Uuid, ServerError> {
    let Some(meta) = meta else {
        return Ok(Uuid::new_v4());
    };
    let request_id = meta_identity(meta, "requestId")?;
    let prompt_id = meta_identity(meta, "promptId")?;
    if request_id.is_some() && prompt_id.is_some() && request_id != prompt_id {
        return Err(ServerError::InvalidRequest(
            "prompt requestId and promptId must match".to_owned(),
        ));
    }
    let Some(value) = request_id.or(prompt_id) else {
        return Ok(Uuid::new_v4());
    };
    Uuid::parse_str(value)
        .map_err(|_| ServerError::InvalidRequest("prompt requestId must be a UUID".to_owned()))
}

fn meta_identity<'a>(meta: &'a Meta, name: &str) -> Result<Option<&'a str>, ServerError> {
    match meta.get(name) {
        None => Ok(None),
        Some(serde_json::Value::String(value)) => Ok(Some(value)),
        Some(_) => Err(ServerError::InvalidRequest(format!(
            "prompt {name} must be a UUID string"
        ))),
    }
}

fn is_compact_invocation(block: &AcpContentBlock) -> bool {
    let AcpContentBlock::Text(text) = block else {
        return false;
    };
    let text = text.text.trim();
    text.strip_prefix("/compact")
        .is_some_and(|tail| tail.is_empty() || tail.starts_with(char::is_whitespace))
}

fn content_block(block: AcpContentBlock) -> Result<ContentBlock, ServerError> {
    match block {
        AcpContentBlock::Text(text) => Ok(ContentBlock::text(text.text)),
        AcpContentBlock::Image(image) => Ok(ContentBlock::image(image.data, image.mime_type)),
        AcpContentBlock::ResourceLink(resource) => Ok(ContentBlock::text(format!(
            "Referenced resource `{}`: {}{}",
            resource.name,
            resource.uri,
            resource
                .description
                .map_or_else(String::new, |description| format!("\n{description}"))
        ))),
        AcpContentBlock::Audio(_) => Err(ServerError::InvalidRequest(
            "audio prompt content is not supported".to_owned(),
        )),
        AcpContentBlock::Resource(_) => Err(ServerError::InvalidRequest(
            "embedded resource prompt content is not supported".to_owned(),
        )),
        _ => Err(ServerError::InvalidRequest(
            "unknown ACP prompt content is not supported".to_owned(),
        )),
    }
}

#[cfg(test)]
mod tests {
    use agent_client_protocol::schema::v1::{
        ContentBlock as AcpContentBlock, ImageContent, TextContent,
    };

    use super::{PromptAction, action};

    #[test]
    fn compact_requires_one_argument_free_text_block() {
        assert!(matches!(
            action(vec![AcpContentBlock::Text(TextContent::new(
                "  /compact\n"
            ))])
            .expect("parse compact"),
            PromptAction::Compact
        ));
        for prompt in [
            vec![AcpContentBlock::Text(TextContent::new("/compact now"))],
            vec![
                AcpContentBlock::Text(TextContent::new("/compact")),
                AcpContentBlock::Image(ImageContent::new("AAEC", "image/png")),
            ],
        ] {
            assert!(
                action(prompt).is_err(),
                "arguments or attachments must not become model-visible text"
            );
        }
    }

    #[test]
    fn similarly_named_slash_commands_remain_normal_prompts() {
        assert!(matches!(
            action(vec![AcpContentBlock::Text(TextContent::new("/compactly"))])
                .expect("parse ordinary prompt"),
            PromptAction::Turn(_)
        ));
    }
}
