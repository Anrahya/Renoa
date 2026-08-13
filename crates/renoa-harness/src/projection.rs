use std::sync::Arc;

use renoa_agent::Message;

use crate::{HarnessError, OperationId, RuntimeProfile, SessionRunLease, store::Store};

pub(crate) async fn project_model_context(
    store: &Store,
    lease: &Arc<SessionRunLease>,
    operation_id: OperationId,
    profile: &RuntimeProfile,
) -> Result<Option<Vec<Message>>, HarnessError> {
    let Some(projector) = &profile.context_projector else {
        return Ok(None);
    };
    let messages = store.load_model_messages(lease, operation_id).await?;
    let cancellation = lease.cancellation();
    let projection = projector.project(messages, cancellation.child_token());
    tokio::pin!(projection);
    tokio::select! {
        biased;
        () = cancellation.cancelled() => Ok(None),
        result = &mut projection => result
            .map(Some)
            .map_err(HarnessError::ContextProjection),
    }
}
