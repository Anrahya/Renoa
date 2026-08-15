use std::{sync::Arc, time::Duration};

use agent_client_protocol::schema::v1::{ContentBlock as AcpContentBlock, Meta, PromptRequest};
use renoa_agent::ContentBlock;
use renoa_harness::{
    CancellationId, HarnessError, OperationOutcome, OperationRequest, RequestId, RunNext,
};
use uuid::Uuid;

use crate::{ServerError, session::ActiveSession};

pub(crate) async fn execute(
    session: &Arc<ActiveSession>,
    request: PromptRequest,
    request_id: RequestId,
    sink: &dyn renoa_agent::AgentEventSink,
) -> Result<OperationOutcome, ServerError> {
    let content = prompt_content(request.prompt)?;
    let lease = session.begin_prompt(request_id).await?;
    let result = execute_admitted(session, content, &lease, sink).await;
    session.finish_prompt(lease.request_id).await;
    result
}

async fn execute_admitted(
    session: &ActiveSession,
    content: Vec<ContentBlock>,
    lease: &crate::session::PromptLease,
    sink: &dyn renoa_agent::AgentEventSink,
) -> Result<OperationOutcome, ServerError> {
    let admission = session
        .harness
        .admit_standalone(session.id, OperationRequest::new(lease.request_id, content))
        .await?;
    if let Some(outcome) = session
        .harness
        .settled_outcome(session.id, admission.operation_id)
        .await?
    {
        return Ok(outcome);
    }

    let execution = session
        .harness
        .run_next_with_events(session.id, &lease.profile, sink);
    tokio::pin!(execution);
    let run = tokio::select! {
        biased;
        result = &mut execution => result?,
        () = lease.cancellation.cancelled() => {
            let cancellation_id = CancellationId::new();
            loop {
                match session.harness.request_standalone_cancellation(
                    session.id,
                    admission.operation_id,
                    cancellation_id,
                ).await {
                    Ok(()) => break execution.await?,
                    Err(HarnessError::OperationNotCancellable(_)) => {
                        if let Ok(result) =
                            tokio::time::timeout(Duration::from_millis(2), &mut execution).await
                        {
                            break result?;
                        }
                    }
                    Err(error) => return Err(error.into()),
                }
            }
        }
    };
    match run {
        RunNext::Finished { outcome, .. } => Ok(outcome),
        RunNext::Blocked { operation_id } => Err(ServerError::Operation(format!(
            "operation {operation_id} is blocked on an uncertain tool outcome"
        ))),
        RunNext::Idle => Err(ServerError::Operation(
            "the admitted operation had no durable outcome".to_owned(),
        )),
        _ => Err(ServerError::Operation(
            "the harness returned an unsupported run result".to_owned(),
        )),
    }
}

pub(crate) fn request_identity(meta: Option<&Meta>) -> Result<RequestId, ServerError> {
    let Some(meta) = meta else {
        return Ok(RequestId::new());
    };
    let request_id = meta_identity(meta, "requestId")?;
    let prompt_id = meta_identity(meta, "promptId")?;
    if request_id.is_some() && prompt_id.is_some() && request_id != prompt_id {
        return Err(ServerError::InvalidRequest(
            "prompt requestId and promptId must match".to_owned(),
        ));
    }
    let Some(value) = request_id.or(prompt_id) else {
        return Ok(RequestId::new());
    };
    Uuid::parse_str(value)
        .map(RequestId::from_uuid)
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
