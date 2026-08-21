use std::sync::Arc;

use agent_client_protocol::schema::v1::{ContentBlock as AcpContentBlock, Meta, PromptRequest};
use renoa_agent::{AgentEventSink, ContentBlock};
use renoa_local::{AlphaSession, LocalTurnOutcome};
use uuid::Uuid;

use crate::ServerError;

pub(crate) async fn execute(
    session: &Arc<AlphaSession>,
    request: PromptRequest,
    request_id: Uuid,
    sink: Arc<dyn AgentEventSink>,
) -> Result<LocalTurnOutcome, ServerError> {
    let content = prompt_content(request.prompt)?;
    Ok(session.execute_turn(request_id, content, sink).await?)
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

fn prompt_content(blocks: Vec<AcpContentBlock>) -> Result<Vec<ContentBlock>, ServerError> {
    if blocks.is_empty() {
        return Err(ServerError::InvalidRequest(
            "prompt must contain at least one content block".to_owned(),
        ));
    }
    blocks.into_iter().map(content_block).collect()
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
