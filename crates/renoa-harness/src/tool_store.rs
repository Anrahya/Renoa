use renoa_agent::{Message, ToolCall, ToolResult};
use rusqlite::{OptionalExtension, TransactionBehavior, params};
use uuid::Uuid;

use crate::store_support::MODEL_ATTEMPT_LIMIT_AFTER_TOOL_RESULTS;
use crate::{
    HarnessError, OperationId, SessionRunLease,
    drive::{PlannedTool, ToolIntent, ToolSettlement, ToolStart},
    schema::{json_error, sqlite_error},
    state::{StoredOperationState, StoredState},
    store::{Store, blocking_transition},
    store_support::{
        advance_entry_cursor, cancellation_requested, finish_active_operation, load_cursors,
        parse_session_id, parse_state, parse_uuid, update_state, validate_tool_batch,
    },
    tool_cancellation_store::{cancel_tool_batch, cancelled_before_execution_result},
};

impl Store {
    pub(crate) async fn load_planned_tool(
        &self,
        lease: &std::sync::Arc<SessionRunLease>,
        operation_id: OperationId,
    ) -> Result<PlannedTool, HarnessError> {
        let database = self.database();
        let lease = std::sync::Arc::clone(lease);
        blocking_transition(lease, move || {
            let connection = database.connection()?;
            let (session_id, state_json) = connection
                .query_row(
                    "SELECT session_id, state_json FROM operations WHERE operation_id = ?1",
                    [operation_id.to_string()],
                    |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
                )
                .optional()
                .map_err(sqlite_error)?
                .ok_or_else(|| HarnessError::Corrupt("active operation is missing".to_owned()))?;
            let state = parse_state(&state_json)?;
            let StoredOperationState::NeedTool { progress, batch } = state.state() else {
                return Err(HarnessError::Corrupt(
                    "planned tool lookup requires NeedTool state".to_owned(),
                ));
            };
            validate_tool_batch(progress, *batch)?;
            let (result_entry_id, call_json, status) = connection
                .query_row(
                    "SELECT result_entry_id, call_json, status FROM tool_calls
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
                        ))
                    },
                )
                .optional()
                .map_err(sqlite_error)?
                .ok_or_else(|| HarnessError::Corrupt("planned tool call is missing".to_owned()))?;
            if status != "planned" {
                return Err(HarnessError::Corrupt(
                    "NeedTool does not reference a planned tool call".to_owned(),
                ));
            }
            let call = serde_json::from_str::<ToolCall>(&call_json).map_err(json_error)?;
            let frozen_tool = progress
                .runtime
                .tools
                .iter()
                .find(|tool| tool.spec.name == call.name)
                .cloned();
            Ok(PlannedTool {
                session_id: parse_session_id(&session_id)?,
                operation_id,
                state_json,
                progress: progress.clone(),
                batch: *batch,
                result_entry_id: parse_uuid(&result_entry_id, "tool result entry id")?,
                call,
                frozen_tool,
            })
        })
        .await
    }

    pub(crate) async fn begin_tool_intent(
        &self,
        lease: &std::sync::Arc<SessionRunLease>,
        planned: PlannedTool,
    ) -> Result<ToolStart, HarnessError> {
        let frozen_tool = planned.frozen_tool.clone().ok_or_else(|| {
            HarnessError::Corrupt("unknown tool cannot begin an external effect".to_owned())
        })?;
        let database = self.database();
        let lease = std::sync::Arc::clone(lease);
        blocking_transition(lease, move || {
            let mut connection = database.connection()?;
            let transaction = connection
                .transaction_with_behavior(TransactionBehavior::Immediate)
                .map_err(sqlite_error)?;
            if cancellation_requested(&transaction, planned.operation_id)? {
                let deleted = transaction
                    .execute(
                        "DELETE FROM tool_calls
                         WHERE operation_id = ?1 AND batch_id = ?2 AND source_index = ?3
                             AND status = 'planned'",
                        params![
                            planned.operation_id.to_string(),
                            planned.batch.batch_id.to_string(),
                            i64::from(planned.batch.next_index),
                        ],
                    )
                    .map_err(sqlite_error)?;
                if deleted != 1 {
                    return Err(HarnessError::Corrupt(
                        "planned tool cancellation compare-and-set failed".to_owned(),
                    ));
                }
                let settlement = cancel_tool_batch(
                    &transaction,
                    ToolResultCommit {
                        session_id: planned.session_id,
                        operation_id: planned.operation_id,
                        progress: planned.progress,
                        batch: planned.batch,
                        result_entry_id: planned.result_entry_id,
                        result: cancelled_before_execution_result(&planned.call),
                        old_state_json: &planned.state_json,
                    },
                )?;
                let ToolSettlement::Finished(outcome) = settlement else {
                    return Err(HarnessError::Corrupt(
                        "planned tool cancellation did not finish the operation".to_owned(),
                    ));
                };
                transaction.commit().map_err(sqlite_error)?;
                return Ok(ToolStart::Finished(outcome));
            }
            let effect_id = Uuid::new_v4();
            let settlement_token = Uuid::new_v4();
            let changed = transaction
                .execute(
                    "UPDATE tool_calls SET status = 'pending', recovery = ?4,
                         effect_id = ?5, settlement_token = ?6
                     WHERE operation_id = ?1 AND batch_id = ?2 AND source_index = ?3
                         AND status = 'planned'",
                    params![
                        planned.operation_id.to_string(),
                        planned.batch.batch_id.to_string(),
                        i64::from(planned.batch.next_index),
                        frozen_tool.recovery.as_str(),
                        effect_id.to_string(),
                        settlement_token.to_string(),
                    ],
                )
                .map_err(sqlite_error)?;
            if changed != 1 {
                return Err(HarnessError::Corrupt(
                    "tool intent compare-and-set failed".to_owned(),
                ));
            }
            let state = StoredState::from_state(StoredOperationState::ToolPending {
                progress: planned.progress.clone(),
                batch: planned.batch,
                effect_id,
                settlement_token,
                recovery: frozen_tool.recovery,
            });
            update_state(
                &transaction,
                planned.operation_id,
                &planned.state_json,
                &serde_json::to_string(&state).map_err(json_error)?,
            )?;
            transaction.commit().map_err(sqlite_error)?;
            Ok(ToolStart::Invoke(Box::new(ToolIntent {
                session_id: planned.session_id,
                operation_id: planned.operation_id,
                progress: planned.progress,
                batch: planned.batch,
                result_entry_id: planned.result_entry_id,
                call: planned.call,
                effect_id,
                settlement_token,
            })))
        })
        .await
    }

    pub(crate) async fn settle_unavailable_tool(
        &self,
        lease: &std::sync::Arc<SessionRunLease>,
        planned: PlannedTool,
        result: ToolResult,
    ) -> Result<ToolSettlement, HarnessError> {
        let database = self.database();
        let lease = std::sync::Arc::clone(lease);
        blocking_transition(lease, move || {
            let mut connection = database.connection()?;
            let transaction = connection
                .transaction_with_behavior(TransactionBehavior::Immediate)
                .map_err(sqlite_error)?;
            let current_state = transaction
                .query_row(
                    "SELECT state_json FROM operations WHERE operation_id = ?1",
                    [planned.operation_id.to_string()],
                    |row| row.get::<_, String>(0),
                )
                .map_err(sqlite_error)?;
            if current_state != planned.state_json {
                return Ok(ToolSettlement::Stale);
            }
            let deleted = transaction
                .execute(
                    "DELETE FROM tool_calls
                     WHERE operation_id = ?1 AND batch_id = ?2 AND source_index = ?3
                         AND status = 'planned'",
                    params![
                        planned.operation_id.to_string(),
                        planned.batch.batch_id.to_string(),
                        i64::from(planned.batch.next_index),
                    ],
                )
                .map_err(sqlite_error)?;
            if deleted != 1 {
                return Err(HarnessError::Corrupt(
                    "unavailable tool settlement compare-and-set failed".to_owned(),
                ));
            }
            let cancelled = cancellation_requested(&transaction, planned.operation_id)?;
            let commit = ToolResultCommit {
                session_id: planned.session_id,
                operation_id: planned.operation_id,
                progress: planned.progress,
                batch: planned.batch,
                result_entry_id: planned.result_entry_id,
                result: if cancelled {
                    cancelled_before_execution_result(&planned.call)
                } else {
                    result
                },
                old_state_json: &planned.state_json,
            };
            let settlement = if cancelled {
                cancel_tool_batch(&transaction, commit)?
            } else {
                append_tool_result_and_advance(&transaction, commit)?
            };
            transaction.commit().map_err(sqlite_error)?;
            Ok(settlement)
        })
        .await
    }

    pub(crate) async fn settle_tool(
        &self,
        lease: &std::sync::Arc<SessionRunLease>,
        intent: ToolIntent,
        result: ToolResult,
    ) -> Result<ToolSettlement, HarnessError> {
        let database = self.database();
        let lease = std::sync::Arc::clone(lease);
        blocking_transition(lease, move || {
            let mut connection = database.connection()?;
            let transaction = connection
                .transaction_with_behavior(TransactionBehavior::Immediate)
                .map_err(sqlite_error)?;
            let Some(old_state_json) = current_tool_state(&transaction, &intent)? else {
                return Ok(ToolSettlement::Stale);
            };
            let deleted = transaction
                .execute(
                    "DELETE FROM tool_calls
                     WHERE operation_id = ?1 AND batch_id = ?2 AND source_index = ?3
                         AND status = 'pending' AND effect_id = ?4 AND settlement_token = ?5",
                    params![
                        intent.operation_id.to_string(),
                        intent.batch.batch_id.to_string(),
                        i64::from(intent.batch.next_index),
                        intent.effect_id.to_string(),
                        intent.settlement_token.to_string(),
                    ],
                )
                .map_err(sqlite_error)?;
            if deleted != 1 {
                return Err(HarnessError::Corrupt(
                    "tool settlement compare-and-set failed".to_owned(),
                ));
            }
            let commit = ToolResultCommit {
                session_id: intent.session_id,
                operation_id: intent.operation_id,
                progress: intent.progress,
                batch: intent.batch,
                result_entry_id: intent.result_entry_id,
                result,
                old_state_json: &old_state_json,
            };
            let settlement = if cancellation_requested(&transaction, intent.operation_id)? {
                cancel_tool_batch(&transaction, commit)?
            } else {
                append_tool_result_and_advance(&transaction, commit)?
            };
            transaction.commit().map_err(sqlite_error)?;
            Ok(settlement)
        })
        .await
    }
}

pub(crate) struct ToolResultCommit<'a> {
    pub(crate) session_id: crate::SessionId,
    pub(crate) operation_id: OperationId,
    pub(crate) progress: crate::state::OperationProgress,
    pub(crate) batch: crate::state::ToolBatch,
    pub(crate) result_entry_id: Uuid,
    pub(crate) result: ToolResult,
    pub(crate) old_state_json: &'a str,
}

fn append_tool_result_and_advance(
    transaction: &rusqlite::Transaction<'_>,
    commit: ToolResultCommit<'_>,
) -> Result<ToolSettlement, HarnessError> {
    let ToolResultCommit {
        session_id,
        operation_id,
        progress,
        batch,
        result_entry_id,
        result,
        old_state_json,
    } = commit;
    let (entry_sequence, output_sequence) = load_cursors(transaction, session_id)?;
    transaction
        .execute(
            "INSERT INTO conversation_entries (
                entry_id, session_id, operation_id, sequence, message_json
             ) VALUES (?1, ?2, ?3, ?4, ?5)",
            params![
                result_entry_id.to_string(),
                session_id.to_string(),
                operation_id.to_string(),
                entry_sequence,
                serde_json::to_string(&Message::Tool { result }).map_err(json_error)?,
            ],
        )
        .map_err(sqlite_error)?;
    let next_index = batch
        .next_index
        .checked_add(1)
        .ok_or_else(|| HarnessError::Corrupt("tool-call index overflowed".to_owned()))?;
    if next_index < batch.call_count {
        let state = StoredState::from_state(StoredOperationState::NeedTool {
            progress,
            batch: crate::state::ToolBatch {
                next_index,
                ..batch
            },
        });
        update_state(
            transaction,
            operation_id,
            old_state_json,
            &serde_json::to_string(&state).map_err(json_error)?,
        )?;
        advance_entry_cursor(transaction, session_id, operation_id, 1)?;
        return Ok(ToolSettlement::Continue(state));
    }
    if progress.model_attempts >= progress.runtime.max_model_attempts {
        let outcome = crate::OperationOutcome::Failed {
            message: MODEL_ATTEMPT_LIMIT_AFTER_TOOL_RESULTS.to_owned(),
        };
        transaction
            .execute(
                "INSERT INTO outputs (
                    output_id, session_id, operation_id, sequence, outcome_json
                 ) VALUES (?1, ?2, ?3, ?4, ?5)",
                params![
                    Uuid::new_v4().to_string(),
                    session_id.to_string(),
                    operation_id.to_string(),
                    output_sequence,
                    serde_json::to_string(&outcome).map_err(json_error)?,
                ],
            )
            .map_err(sqlite_error)?;
        let state = StoredState::from_state(StoredOperationState::Failed {
            kind: crate::state::FailureKind::General,
        });
        update_state(
            transaction,
            operation_id,
            old_state_json,
            &serde_json::to_string(&state).map_err(json_error)?,
        )?;
        finish_active_operation(transaction, session_id, operation_id, 1)?;
        return Ok(ToolSettlement::Finished(outcome));
    }
    let state = StoredState::from_state(StoredOperationState::NeedModel { progress });
    update_state(
        transaction,
        operation_id,
        old_state_json,
        &serde_json::to_string(&state).map_err(json_error)?,
    )?;
    advance_entry_cursor(transaction, session_id, operation_id, 1)?;
    Ok(ToolSettlement::Continue(state))
}

pub(crate) fn current_tool_state(
    transaction: &rusqlite::Transaction<'_>,
    intent: &ToolIntent,
) -> Result<Option<String>, HarnessError> {
    let state_json = transaction
        .query_row(
            "SELECT state_json FROM operations WHERE operation_id = ?1",
            [intent.operation_id.to_string()],
            |row| row.get::<_, String>(0),
        )
        .optional()
        .map_err(sqlite_error)?
        .ok_or_else(|| HarnessError::Corrupt("active operation is missing".to_owned()))?;
    let state = parse_state(&state_json)?;
    match state.state() {
        StoredOperationState::ToolPending {
            effect_id,
            settlement_token,
            ..
        } if *effect_id == intent.effect_id && *settlement_token == intent.settlement_token => {
            Ok(Some(state_json))
        }
        _ => Ok(None),
    }
}
