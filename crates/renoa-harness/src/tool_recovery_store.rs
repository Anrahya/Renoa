use renoa_agent::ToolCall;
use rusqlite::{OptionalExtension, Transaction, TransactionBehavior, params};
use uuid::Uuid;

use crate::{
    HarnessError, OperationId, SessionId, SessionRunLease,
    drive::{ToolIntent, ToolPendingRecovery},
    schema::{json_error, sqlite_error},
    state::{OperationProgress, StoredOperationState, StoredState, ToolBatch, ToolRecovery},
    store::{Store, blocking_transition},
    store_support::{
        cancellation_requested, parse_session_id, parse_state, parse_uuid, update_state,
        validate_tool_batch,
    },
};

impl Store {
    pub(crate) async fn recover_tool_attempt(
        &self,
        lease: &std::sync::Arc<SessionRunLease>,
        operation_id: OperationId,
    ) -> Result<ToolPendingRecovery, HarnessError> {
        let database = self.database();
        let lease = std::sync::Arc::clone(lease);
        blocking_transition(lease, move || {
            let mut connection = database.connection()?;
            let transaction = connection
                .transaction_with_behavior(TransactionBehavior::Immediate)
                .map_err(sqlite_error)?;
            let pending = load_pending_tool(&transaction, operation_id)?;
            let recovery = if cancellation_requested(&transaction, operation_id)? {
                mark_tool_outcome_unknown(&transaction, operation_id, &pending)?;
                ToolPendingRecovery::Blocked
            } else {
                match pending.recovery {
                    ToolRecovery::SafeToReplay => ToolPendingRecovery::Retry(Box::new(
                        retry_safe_tool(&transaction, operation_id, pending)?,
                    )),
                    ToolRecovery::NeverReplay => {
                        mark_tool_outcome_unknown(&transaction, operation_id, &pending)?;
                        ToolPendingRecovery::Blocked
                    }
                }
            };
            transaction.commit().map_err(sqlite_error)?;
            Ok(recovery)
        })
        .await
    }
}

struct PendingTool {
    session_id: SessionId,
    state_json: String,
    progress: OperationProgress,
    batch: ToolBatch,
    effect_id: Uuid,
    settlement_token: Uuid,
    recovery: ToolRecovery,
    result_entry_id: Uuid,
    call: ToolCall,
}

fn load_pending_tool(
    transaction: &Transaction<'_>,
    operation_id: OperationId,
) -> Result<PendingTool, HarnessError> {
    let (session_id, state_json) = transaction
        .query_row(
            "SELECT session_id, state_json FROM operations WHERE operation_id = ?1",
            [operation_id.to_string()],
            |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
        )
        .optional()
        .map_err(sqlite_error)?
        .ok_or_else(|| HarnessError::Corrupt("active operation is missing".to_owned()))?;
    let state = parse_state(&state_json)?;
    let StoredOperationState::ToolPending {
        progress,
        batch,
        effect_id,
        settlement_token,
        recovery,
    } = state.state()
    else {
        return Err(HarnessError::Corrupt(
            "tool recovery requires ToolPending state".to_owned(),
        ));
    };
    validate_tool_batch(progress, *batch)?;
    let (result_entry_id, call, status, stored_recovery, stored_effect, stored_token) = transaction
        .query_row(
            "SELECT result_entry_id, call_json, status, recovery,
                    effect_id, settlement_token
             FROM tool_calls
             WHERE operation_id = ?1 AND batch_id = ?2 AND source_index = ?3",
            params![
                operation_id.to_string(),
                batch.batch_id.to_string(),
                i64::from(batch.next_index),
            ],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, String>(4)?,
                    row.get::<_, String>(5)?,
                ))
            },
        )
        .optional()
        .map_err(sqlite_error)?
        .ok_or_else(|| HarnessError::Corrupt("pending tool call is missing".to_owned()))?;
    if status != "pending"
        || stored_recovery != recovery.as_str()
        || stored_effect != effect_id.to_string()
        || stored_token != settlement_token.to_string()
    {
        return Err(HarnessError::Corrupt(
            "pending tool call does not match operation state".to_owned(),
        ));
    }
    let call = serde_json::from_str::<ToolCall>(&call).map_err(json_error)?;
    let frozen_recovery = progress
        .runtime
        .tools
        .iter()
        .find(|tool| tool.spec.name == call.name)
        .map(|tool| tool.recovery)
        .ok_or_else(|| {
            HarnessError::Corrupt("pending tool is absent from the frozen profile".to_owned())
        })?;
    if frozen_recovery != *recovery {
        return Err(HarnessError::Corrupt(
            "pending tool recovery differs from the frozen profile".to_owned(),
        ));
    }
    Ok(PendingTool {
        session_id: parse_session_id(&session_id)?,
        state_json,
        progress: progress.clone(),
        batch: *batch,
        effect_id: *effect_id,
        settlement_token: *settlement_token,
        recovery: *recovery,
        result_entry_id: parse_uuid(&result_entry_id, "tool result entry id")?,
        call,
    })
}

fn retry_safe_tool(
    transaction: &Transaction<'_>,
    operation_id: OperationId,
    pending: PendingTool,
) -> Result<ToolIntent, HarnessError> {
    let effect_id = Uuid::new_v4();
    let settlement_token = Uuid::new_v4();
    let changed = transaction
        .execute(
            "UPDATE tool_calls SET effect_id = ?4, settlement_token = ?5
             WHERE operation_id = ?1 AND batch_id = ?2 AND source_index = ?3
                 AND status = 'pending' AND effect_id = ?6 AND settlement_token = ?7",
            params![
                operation_id.to_string(),
                pending.batch.batch_id.to_string(),
                i64::from(pending.batch.next_index),
                effect_id.to_string(),
                settlement_token.to_string(),
                pending.effect_id.to_string(),
                pending.settlement_token.to_string(),
            ],
        )
        .map_err(sqlite_error)?;
    if changed != 1 {
        return Err(HarnessError::Corrupt(
            "safe tool recovery compare-and-set failed".to_owned(),
        ));
    }
    let next_state = StoredState::from_state(StoredOperationState::ToolPending {
        progress: pending.progress.clone(),
        batch: pending.batch,
        effect_id,
        settlement_token,
        recovery: pending.recovery,
    });
    update_state(
        transaction,
        operation_id,
        &pending.state_json,
        &serde_json::to_string(&next_state).map_err(json_error)?,
    )?;
    Ok(ToolIntent {
        session_id: pending.session_id,
        operation_id,
        progress: pending.progress,
        batch: pending.batch,
        result_entry_id: pending.result_entry_id,
        call: pending.call,
        effect_id,
        settlement_token,
    })
}

fn mark_tool_outcome_unknown(
    transaction: &Transaction<'_>,
    operation_id: OperationId,
    pending: &PendingTool,
) -> Result<(), HarnessError> {
    let changed = transaction
        .execute(
            "UPDATE tool_calls SET status = 'outcome_unknown'
             WHERE operation_id = ?1 AND batch_id = ?2 AND source_index = ?3
                 AND status = 'pending' AND effect_id = ?4 AND settlement_token = ?5",
            params![
                operation_id.to_string(),
                pending.batch.batch_id.to_string(),
                i64::from(pending.batch.next_index),
                pending.effect_id.to_string(),
                pending.settlement_token.to_string(),
            ],
        )
        .map_err(sqlite_error)?;
    if changed != 1 {
        return Err(HarnessError::Corrupt(
            "unsafe tool recovery compare-and-set failed".to_owned(),
        ));
    }
    let next_state = StoredState::from_state(StoredOperationState::ToolOutcomeUnknown {
        progress: pending.progress.clone(),
        batch: pending.batch,
    });
    update_state(
        transaction,
        operation_id,
        &pending.state_json,
        &serde_json::to_string(&next_state).map_err(json_error)?,
    )
}
