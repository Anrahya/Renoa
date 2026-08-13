use renoa_agent::{ContentBlock, Message, ToolCall, ToolResult};
use rusqlite::{OptionalExtension, Transaction, TransactionBehavior, params};
use uuid::Uuid;

use crate::{
    HarnessError, OperationId, OperationOutcome, SessionId, SessionRunLease,
    schema::{json_error, sqlite_error},
    state::{FailureKind, StoredOperationState, StoredState, ToolBatch},
    store::{Store, blocking_transition},
    store_support::{finish_active_operation, parse_state, update_state, validate_tool_batch},
};

const ABANDONED_MESSAGE: &str =
    "tool outcome is unknown; the operation was abandoned without replay";

impl Store {
    pub(crate) async fn abandon_unknown_tool(
        &self,
        lease: &std::sync::Arc<SessionRunLease>,
        session_id: SessionId,
        operation_id: OperationId,
    ) -> Result<OperationOutcome, HarnessError> {
        let database = self.database();
        let lease = std::sync::Arc::clone(lease);
        blocking_transition(lease, move || {
            let mut connection = database.connection()?;
            let transaction = connection
                .transaction_with_behavior(TransactionBehavior::Immediate)
                .map_err(sqlite_error)?;
            let outcome = abandon_unknown_transaction(&transaction, session_id, operation_id)?;
            transaction.commit().map_err(sqlite_error)?;
            Ok(outcome)
        })
        .await
    }
}

fn abandon_unknown_transaction(
    transaction: &Transaction<'_>,
    session_id: SessionId,
    operation_id: OperationId,
) -> Result<OperationOutcome, HarnessError> {
    let UnknownToolState::Pending { state_json, batch } =
        load_unknown_tool_state(transaction, session_id, operation_id)?
    else {
        return load_outcome(transaction, operation_id);
    };
    let cursors = load_active_session(transaction, session_id, operation_id)?;
    let calls = load_unresolved_calls(transaction, operation_id, batch)?;
    append_abandonment_results(
        transaction,
        session_id,
        operation_id,
        cursors.next_entry_sequence,
        &calls,
    )?;
    let deleted = transaction
        .execute(
            "DELETE FROM tool_calls WHERE operation_id = ?1 AND batch_id = ?2",
            params![operation_id.to_string(), batch.batch_id.to_string()],
        )
        .map_err(sqlite_error)?;
    if deleted != calls.len() {
        return Err(HarnessError::Corrupt(
            "unknown tool batch changed during abandonment".to_owned(),
        ));
    }
    let outcome = OperationOutcome::Failed {
        message: ABANDONED_MESSAGE.to_owned(),
    };
    insert_abandonment_output(
        transaction,
        session_id,
        operation_id,
        cursors.next_output_sequence,
        &outcome,
    )?;
    let terminal = StoredState::from_state(StoredOperationState::Failed {
        kind: FailureKind::AbandonedUnknownTool,
    });
    update_state(
        transaction,
        operation_id,
        &state_json,
        &serde_json::to_string(&terminal).map_err(json_error)?,
    )?;
    let result_count = i64::try_from(calls.len())
        .map_err(|_| HarnessError::Corrupt("tool-result count exceeds i64".to_owned()))?;
    finish_active_operation(transaction, session_id, operation_id, result_count)?;
    Ok(outcome)
}

enum UnknownToolState {
    Pending {
        state_json: String,
        batch: ToolBatch,
    },
    AlreadyAbandoned,
}

fn load_unknown_tool_state(
    transaction: &Transaction<'_>,
    session_id: SessionId,
    operation_id: OperationId,
) -> Result<UnknownToolState, HarnessError> {
    let state_json = transaction
        .query_row(
            "SELECT state_json FROM operations WHERE session_id = ?1 AND operation_id = ?2",
            params![session_id.to_string(), operation_id.to_string()],
            |row| row.get::<_, String>(0),
        )
        .optional()
        .map_err(sqlite_error)?
        .ok_or(HarnessError::NoUnknownToolOutcome(operation_id))?;
    let state = parse_state(&state_json)?;
    match state.state() {
        StoredOperationState::ToolOutcomeUnknown { progress, batch } => {
            validate_tool_batch(progress, *batch)?;
            Ok(UnknownToolState::Pending {
                state_json,
                batch: *batch,
            })
        }
        StoredOperationState::Failed {
            kind: FailureKind::AbandonedUnknownTool,
        } => Ok(UnknownToolState::AlreadyAbandoned),
        _ => Err(HarnessError::NoUnknownToolOutcome(operation_id)),
    }
}

struct SessionCursors {
    next_entry_sequence: i64,
    next_output_sequence: i64,
}

fn load_active_session(
    transaction: &Transaction<'_>,
    session_id: SessionId,
    operation_id: OperationId,
) -> Result<SessionCursors, HarnessError> {
    let (active_operation, next_entry_sequence, next_output_sequence) = transaction
        .query_row(
            "SELECT active_operation_id, next_entry_sequence, next_output_sequence
             FROM sessions WHERE session_id = ?1",
            [session_id.to_string()],
            |row| {
                Ok((
                    row.get::<_, Option<String>>(0)?,
                    row.get::<_, i64>(1)?,
                    row.get::<_, i64>(2)?,
                ))
            },
        )
        .optional()
        .map_err(sqlite_error)?
        .ok_or(HarnessError::SessionNotFound(session_id))?;
    let operation_id = operation_id.to_string();
    if active_operation.as_deref() != Some(operation_id.as_str()) {
        return Err(HarnessError::Corrupt(
            "unknown tool operation is not the session's active operation".to_owned(),
        ));
    }
    Ok(SessionCursors {
        next_entry_sequence,
        next_output_sequence,
    })
}

struct UnresolvedCall {
    result_entry_id: String,
    call: ToolCall,
}

fn load_unresolved_calls(
    transaction: &Transaction<'_>,
    operation_id: OperationId,
    batch: ToolBatch,
) -> Result<Vec<UnresolvedCall>, HarnessError> {
    let mut statement = transaction
        .prepare(
            "SELECT source_index, result_entry_id, call_json, status
             FROM tool_calls WHERE operation_id = ?1 AND batch_id = ?2
                 AND source_index >= ?3 ORDER BY source_index",
        )
        .map_err(sqlite_error)?;
    let rows = statement
        .query_map(
            params![
                operation_id.to_string(),
                batch.batch_id.to_string(),
                i64::from(batch.next_index),
            ],
            |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                ))
            },
        )
        .map_err(sqlite_error)?;
    let mut calls = Vec::new();
    for (offset, row) in rows.enumerate() {
        let (source_index, result_entry_id, call_json, status) = row.map_err(sqlite_error)?;
        validate_unresolved_row(batch, offset, source_index, &status)?;
        calls.push(UnresolvedCall {
            result_entry_id,
            call: serde_json::from_str(&call_json).map_err(json_error)?,
        });
    }
    let remaining = batch
        .call_count
        .checked_sub(batch.next_index)
        .ok_or_else(|| {
            HarnessError::Corrupt("tool batch cursor exceeds its call count".to_owned())
        })?;
    let expected_count = usize::try_from(remaining)
        .map_err(|_| HarnessError::Corrupt("tool-call count exceeds usize".to_owned()))?;
    if calls.len() != expected_count {
        return Err(HarnessError::Corrupt(
            "unknown tool batch has missing calls".to_owned(),
        ));
    }
    Ok(calls)
}

fn validate_unresolved_row(
    batch: ToolBatch,
    offset: usize,
    source_index: i64,
    status: &str,
) -> Result<(), HarnessError> {
    let expected_index = i64::from(batch.next_index)
        .checked_add(
            i64::try_from(offset)
                .map_err(|_| HarnessError::Corrupt("tool-call offset exceeds i64".to_owned()))?,
        )
        .ok_or_else(|| HarnessError::Corrupt("tool-call index overflowed".to_owned()))?;
    let expected_status = if offset == 0 {
        "outcome_unknown"
    } else {
        "planned"
    };
    if source_index != expected_index || status != expected_status {
        return Err(HarnessError::Corrupt(
            "unknown tool batch is incomplete or out of order".to_owned(),
        ));
    }
    Ok(())
}

fn append_abandonment_results(
    transaction: &Transaction<'_>,
    session_id: SessionId,
    operation_id: OperationId,
    first_sequence: i64,
    calls: &[UnresolvedCall],
) -> Result<(), HarnessError> {
    for (offset, call) in calls.iter().enumerate() {
        let offset = i64::try_from(offset)
            .map_err(|_| HarnessError::Corrupt("tool-result offset exceeds i64".to_owned()))?;
        let sequence = first_sequence
            .checked_add(offset)
            .ok_or_else(|| HarnessError::Corrupt("entry cursor overflowed".to_owned()))?;
        let message = if offset == 0 {
            "Tool outcome is unknown after restart; it was not retried."
        } else {
            "Tool call was not executed because an earlier tool outcome is unknown."
        };
        let result = ToolResult {
            call_id: call.call.id.clone(),
            name: call.call.name.clone(),
            content: vec![ContentBlock::text(message)],
            details: None,
            is_error: true,
        };
        transaction
            .execute(
                "INSERT INTO conversation_entries (
                    entry_id, session_id, operation_id, sequence, message_json
                 ) VALUES (?1, ?2, ?3, ?4, ?5)",
                params![
                    call.result_entry_id,
                    session_id.to_string(),
                    operation_id.to_string(),
                    sequence,
                    serde_json::to_string(&Message::Tool { result }).map_err(json_error)?,
                ],
            )
            .map_err(sqlite_error)?;
    }
    Ok(())
}

fn insert_abandonment_output(
    transaction: &Transaction<'_>,
    session_id: SessionId,
    operation_id: OperationId,
    sequence: i64,
    outcome: &OperationOutcome,
) -> Result<(), HarnessError> {
    transaction
        .execute(
            "INSERT INTO outputs (output_id, session_id, operation_id, sequence, outcome_json)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![
                Uuid::new_v4().to_string(),
                session_id.to_string(),
                operation_id.to_string(),
                sequence,
                serde_json::to_string(outcome).map_err(json_error)?,
            ],
        )
        .map_err(sqlite_error)?;
    Ok(())
}

fn load_outcome(
    transaction: &Transaction<'_>,
    operation_id: OperationId,
) -> Result<OperationOutcome, HarnessError> {
    let outcome_json = transaction
        .query_row(
            "SELECT outcome_json FROM outputs WHERE operation_id = ?1",
            [operation_id.to_string()],
            |row| row.get::<_, String>(0),
        )
        .optional()
        .map_err(sqlite_error)?
        .ok_or_else(|| HarnessError::Corrupt("abandoned operation output is missing".to_owned()))?;
    serde_json::from_str(&outcome_json).map_err(json_error)
}
