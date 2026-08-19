use rusqlite::{Transaction, TransactionBehavior, params};

use crate::{
    EventId, Kernel, KernelError, LoopDecision, OperationId, OperationOutcome, SessionId,
    operation_phase::OperationPhase,
    schema::{json_error, sqlite_error},
};

pub(crate) enum CommittedDecision {
    Continue,
    Finished(OperationOutcome),
}

impl Kernel {
    pub(crate) fn commit_decision(
        &self,
        session_id: SessionId,
        operation_id: OperationId,
        expected_transition: i64,
        decision: LoopDecision,
    ) -> Result<CommittedDecision, KernelError> {
        let (checkpoint, events, phase, outcome) = match decision {
            LoopDecision::AppendEventsAndContinue { checkpoint, events } => {
                if events.is_empty() {
                    return Err(KernelError::InvalidDecision(
                        "append-and-continue requires at least one event".to_owned(),
                    ));
                }
                (checkpoint, events, OperationPhase::NeedDecision, None)
            }
            LoopDecision::WaitForInput { checkpoint, events } => (
                checkpoint,
                events,
                OperationPhase::Waiting,
                Some(OperationOutcome::WaitingForInput),
            ),
            LoopDecision::Complete { checkpoint, events } => (
                checkpoint,
                events,
                OperationPhase::Completed,
                Some(OperationOutcome::Completed),
            ),
            LoopDecision::Fail {
                checkpoint,
                events,
                reason,
            } => (
                checkpoint,
                events,
                OperationPhase::Failed,
                Some(OperationOutcome::Failed { reason }),
            ),
            LoopDecision::InvokeEffect { binding, .. } => {
                return Err(KernelError::EffectBindingUnavailable(binding));
            }
        };
        if events.iter().any(|event| event.kind.is_empty()) {
            return Err(KernelError::InvalidDecision(
                "semantic event kind cannot be empty".to_owned(),
            ));
        }
        let mut connection = self.database.connection()?;
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(sqlite_error)?;
        append_events(&transaction, session_id, operation_id, &events)?;
        let checkpoint_json = serde_json::to_string(&checkpoint).map_err(json_error)?;
        let outcome_json = outcome
            .as_ref()
            .map(serde_json::to_string)
            .transpose()
            .map_err(json_error)?;
        let changed = transaction
            .execute(
                "UPDATE operations
                 SET phase = ?3, checkpoint_json = ?4, outcome_json = ?5,
                     input_effect_id = NULL,
                     transition_version = transition_version + 1
                 WHERE operation_id = ?1 AND phase = 'need_decision'
                     AND transition_version = ?2",
                params![
                    operation_id.to_string(),
                    expected_transition,
                    phase.as_str(),
                    checkpoint_json,
                    outcome_json,
                ],
            )
            .map_err(sqlite_error)?;
        if changed != 1 {
            return Err(KernelError::Corrupt(
                "loop decision compare-and-set failed".to_owned(),
            ));
        }
        if outcome.is_some() {
            let changed = transaction
                .execute(
                    "UPDATE sessions SET active_operation_id = NULL
                     WHERE session_id = ?1 AND active_operation_id = ?2",
                    params![session_id.to_string(), operation_id.to_string()],
                )
                .map_err(sqlite_error)?;
            if changed != 1 {
                return Err(KernelError::Corrupt(
                    "terminal operation did not own its session".to_owned(),
                ));
            }
        }
        transaction.commit().map_err(sqlite_error)?;
        Ok(match outcome {
            Some(outcome) => CommittedDecision::Finished(outcome),
            None => CommittedDecision::Continue,
        })
    }
}

pub(crate) fn append_events(
    transaction: &Transaction<'_>,
    session_id: SessionId,
    operation_id: OperationId,
    events: &[crate::NewEvent],
) -> Result<(), KernelError> {
    let first_sequence = transaction
        .query_row(
            "SELECT next_event_sequence FROM sessions WHERE session_id = ?1",
            [session_id.to_string()],
            |row| row.get::<_, i64>(0),
        )
        .map_err(sqlite_error)?;
    for (offset, event) in events.iter().enumerate() {
        let offset = i64::try_from(offset)
            .map_err(|error| KernelError::Corrupt(format!("event offset exceeds i64: {error}")))?;
        let sequence = first_sequence
            .checked_add(offset)
            .ok_or_else(|| KernelError::Corrupt("event sequence overflowed".to_owned()))?;
        transaction
            .execute(
                "INSERT INTO semantic_events (
                    event_id, session_id, operation_id, sequence, kind, payload_json
                 ) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                params![
                    EventId::new().to_string(),
                    session_id.to_string(),
                    operation_id.to_string(),
                    sequence,
                    &event.kind,
                    serde_json::to_string(&event.payload).map_err(json_error)?,
                ],
            )
            .map_err(sqlite_error)?;
    }
    let count = i64::try_from(events.len())
        .map_err(|error| KernelError::Corrupt(format!("event count exceeds i64: {error}")))?;
    let changed = transaction
        .execute(
            "UPDATE sessions SET next_event_sequence = next_event_sequence + ?2
             WHERE session_id = ?1 AND next_event_sequence = ?3",
            params![session_id.to_string(), count, first_sequence],
        )
        .map_err(sqlite_error)?;
    if changed != 1 {
        return Err(KernelError::Corrupt(
            "event cursor compare-and-set failed".to_owned(),
        ));
    }
    Ok(())
}
