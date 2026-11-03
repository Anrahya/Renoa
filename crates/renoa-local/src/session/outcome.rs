//! What a settled operation returns to its caller, read from the events it
//! committed: its last reply, a compaction's result, or the question it
//! waits on.

use renoa_agent::{AssistantContent, Message, StopReason};
use renoa_agent_loop::{
    COMPACTION_RESULT_EVENT_KIND, CONTEXT_CHECKPOINT_EVENT_KIND, CompactionResult,
    MESSAGE_EVENT_KIND,
};
use renoa_kernel::{EventCursor, OperationId, OperationOutcome, SemanticEvent};

use super::{LocalSession, LocalSessionError, LocalTurnOutcome};

impl LocalSession {
    pub(super) fn project_outcome(
        &self,
        operation_id: OperationId,
        outcome: OperationOutcome,
    ) -> Result<LocalTurnOutcome, LocalSessionError> {
        match outcome {
            OperationOutcome::Completed => self.completed_outcome(operation_id),
            OperationOutcome::Cancelled => Ok(LocalTurnOutcome::Cancelled),
            OperationOutcome::Failed { reason } => Ok(LocalTurnOutcome::Failed { reason }),
            OperationOutcome::WaitingForInput => {
                let events = self.operation_events(operation_id)?;
                let (output, _) = last_reply(operation_id, &events)?;
                Ok(LocalTurnOutcome::WaitingForInput { output })
            }
            _ => Err(LocalSessionError::UnsupportedOperationOutcome),
        }
    }

    fn completed_outcome(
        &self,
        operation_id: OperationId,
    ) -> Result<LocalTurnOutcome, LocalSessionError> {
        let operation_events = self.operation_events(operation_id)?;
        let compaction_results = operation_events
            .iter()
            .filter(|event| event.kind == COMPACTION_RESULT_EVENT_KIND)
            .collect::<Vec<_>>();
        if compaction_results.len() > 1 {
            return Err(LocalSessionError::DuplicateCompactionResult(operation_id));
        }
        if let Some(event) = compaction_results.first() {
            if operation_events
                .iter()
                .any(|event| event.kind == MESSAGE_EVENT_KIND)
            {
                return Err(LocalSessionError::MixedCompletedResult(operation_id));
            }
            let result = decode_compaction_result(operation_id, event.payload.clone())?;
            return Ok(LocalTurnOutcome::Compacted {
                estimated_input_tokens: result.estimated_input_tokens(),
                summarized: operation_events
                    .iter()
                    .any(|event| event.kind == CONTEXT_CHECKPOINT_EVENT_KIND),
            });
        }
        let (output, stop_reason) = last_reply(operation_id, &operation_events)?;
        Ok(LocalTurnOutcome::Completed {
            output,
            stop_reason,
        })
    }

    fn operation_events(
        &self,
        operation_id: OperationId,
    ) -> Result<Vec<SemanticEvent>, LocalSessionError> {
        let page = self
            .kernel
            .events_after(self.session_id, EventCursor::START)?;
        Ok(page
            .events
            .into_iter()
            .filter(|event| event.operation_id == operation_id)
            .collect())
    }
}

/// The text of an operation's last assistant message and its stop reason.
fn last_reply(
    operation_id: OperationId,
    operation_events: &[SemanticEvent],
) -> Result<(String, StopReason), LocalSessionError> {
    let event = operation_events
        .iter()
        .rev()
        .find(|event| event.kind == MESSAGE_EVENT_KIND)
        .ok_or(LocalSessionError::MessageMissing(operation_id))?;
    let message = serde_json::from_value::<Message>(event.payload.clone()).map_err(|source| {
        LocalSessionError::MessageInvalid {
            operation_id,
            source,
        }
    })?;
    let Message::Assistant {
        content,
        stop_reason,
        ..
    } = message
    else {
        return Err(LocalSessionError::NonAssistantMessage(operation_id));
    };
    let output = content
        .into_iter()
        .filter_map(|block| match block {
            AssistantContent::Text { text, .. } => Some(text),
            AssistantContent::Reasoning { .. } | AssistantContent::ToolCall { .. } => None,
        })
        .collect();
    Ok((output, stop_reason))
}

pub(super) fn decode_compaction_result(
    operation_id: OperationId,
    payload: serde_json::Value,
) -> Result<CompactionResult, LocalSessionError> {
    serde_json::from_value(payload).map_err(|source| LocalSessionError::CompactionResultInvalid {
        operation_id,
        source,
    })
}
