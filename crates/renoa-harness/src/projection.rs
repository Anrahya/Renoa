use std::sync::Arc;

use renoa_agent::ModelRequest;

use crate::{
    HarnessError, OperationId, RuntimeProfile, SessionRunLease, state::OperationProgress,
    store::Store,
};

pub(crate) async fn project_model_request(
    store: &Store,
    lease: &Arc<SessionRunLease>,
    operation_id: OperationId,
    progress: &OperationProgress,
    profile: &RuntimeProfile,
) -> Result<Option<ModelRequest>, HarnessError> {
    let messages = store.load_model_messages(lease, operation_id).await?;
    let Some(projector) = &profile.context_projector else {
        return Ok(Some(request(progress, messages)));
    };
    let cancellation = lease.cancellation();
    let projection = projector.project(messages, cancellation.child_token());
    tokio::pin!(projection);
    tokio::select! {
        biased;
        () = cancellation.cancelled() => Ok(None),
        result = &mut projection => result
            .map(|messages| Some(request(progress, messages)))
            .map_err(HarnessError::ContextProjection),
    }
}

fn request(progress: &OperationProgress, messages: Vec<renoa_agent::Message>) -> ModelRequest {
    ModelRequest {
        system_prompt: progress.runtime.system_prompt.clone(),
        messages,
        tools: progress
            .runtime
            .tools
            .iter()
            .map(|tool| tool.spec.clone())
            .collect(),
    }
}
