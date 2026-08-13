use renoa_agent::{
    AssistantContent, ContentBlock, Message, ModelResponse, StopReason, TokenUsage, ToolCall,
    ToolResult,
};
use rusqlite::{TransactionBehavior, params};
use uuid::Uuid;

use crate::{
    HarnessError, OperationOutcome, SessionRunLease,
    drive::{ModelIntent, Settlement},
    schema::{json_error, sqlite_error},
    state::{StoredOperationState, StoredState, ToolBatch},
    store::{Store, blocking_transition},
    store_support::{
        MODEL_ATTEMPT_LIMIT_AFTER_TOOL_RESULTS, add_token_usage, advance_entry_cursor,
        cancellation_requested, current_pending_state, finish_active_operation,
        finish_cancelled_operation, finish_failed_operation, insert_output, load_cursors,
        update_state,
    },
};

impl Store {
    pub(crate) async fn settle_model(
        &self,
        lease: &std::sync::Arc<SessionRunLease>,
        intent: ModelIntent,
        response: ModelResponse,
    ) -> Result<Settlement, HarnessError> {
        let database = self.database();
        let lease = std::sync::Arc::clone(lease);
        blocking_transition(lease, move || {
            let mut connection = database.connection()?;
            let transaction = connection
                .transaction_with_behavior(TransactionBehavior::Immediate)
                .map_err(sqlite_error)?;
            let settlement = settle_model_transaction(&transaction, &intent, response)?;
            transaction.commit().map_err(sqlite_error)?;
            Ok(settlement)
        })
        .await
    }

    pub(crate) async fn reject_model_response(
        &self,
        lease: &std::sync::Arc<SessionRunLease>,
        intent: ModelIntent,
        usage: Option<TokenUsage>,
        message: String,
    ) -> Result<Settlement, HarnessError> {
        let database = self.database();
        let lease = std::sync::Arc::clone(lease);
        blocking_transition(lease, move || {
            let mut connection = database.connection()?;
            let transaction = connection
                .transaction_with_behavior(TransactionBehavior::Immediate)
                .map_err(sqlite_error)?;
            let Some(old_state_json) = current_pending_state(&transaction, &intent)? else {
                return Ok(Settlement::Stale);
            };
            if cancellation_requested(&transaction, intent.operation_id)? {
                let settlement = cancel_completed_model_response(
                    &transaction,
                    &intent,
                    &old_state_json,
                    usage,
                    Some(&message),
                )?;
                transaction.commit().map_err(sqlite_error)?;
                return Ok(settlement);
            }
            let changed = transaction
                .execute(
                    "UPDATE model_attempts
                     SET status = 'completed', request_json = NULL,
                         usage_json = ?3, error = ?4
                     WHERE effect_id = ?1 AND settlement_token = ?2 AND status = 'pending'",
                    params![
                        intent.effect_id.to_string(),
                        intent.settlement_token.to_string(),
                        serde_json::to_string(&usage).map_err(json_error)?,
                        message,
                    ],
                )
                .map_err(sqlite_error)?;
            if changed != 1 {
                return Err(HarnessError::Corrupt(
                    "invalid model response compare-and-set failed".to_owned(),
                ));
            }
            let outcome = finish_failed_operation(&transaction, &intent, &old_state_json, message)?;
            transaction.commit().map_err(sqlite_error)?;
            Ok(Settlement::Applied(outcome))
        })
        .await
    }
}

struct CommittedResponse {
    calls: Vec<ToolCall>,
    output: String,
    stop_reason: StopReason,
    entry_sequence: i64,
    output_sequence: i64,
}

fn settle_model_transaction(
    transaction: &rusqlite::Transaction<'_>,
    intent: &ModelIntent,
    response: ModelResponse,
) -> Result<Settlement, HarnessError> {
    let Some(old_state_json) = current_pending_state(transaction, intent)? else {
        return Ok(Settlement::Stale);
    };
    if cancellation_requested(transaction, intent.operation_id)? {
        return cancel_completed_model_response(
            transaction,
            intent,
            &old_state_json,
            response.usage,
            None,
        );
    }
    let committed = commit_model_response(transaction, intent, response)?;
    classify_committed_response(transaction, intent, &old_state_json, committed)
}

fn cancel_completed_model_response(
    transaction: &rusqlite::Transaction<'_>,
    intent: &ModelIntent,
    old_state_json: &str,
    usage: Option<TokenUsage>,
    error: Option<&str>,
) -> Result<Settlement, HarnessError> {
    let changed = transaction
        .execute(
            "UPDATE model_attempts
             SET status = 'completed', request_json = NULL, usage_json = ?3, error = ?4
             WHERE effect_id = ?1 AND settlement_token = ?2 AND status = 'pending'",
            params![
                intent.effect_id.to_string(),
                intent.settlement_token.to_string(),
                serde_json::to_string(&usage).map_err(json_error)?,
                error,
            ],
        )
        .map_err(sqlite_error)?;
    if changed != 1 {
        return Err(HarnessError::Corrupt(
            "cancelled model response compare-and-set failed".to_owned(),
        ));
    }
    let outcome = finish_cancelled_operation(
        transaction,
        intent.session_id,
        intent.operation_id,
        intent.output_id,
        old_state_json,
        0,
    )?;
    Ok(Settlement::Applied(outcome))
}

fn commit_model_response(
    transaction: &rusqlite::Transaction<'_>,
    intent: &ModelIntent,
    response: ModelResponse,
) -> Result<CommittedResponse, HarnessError> {
    let (entry_sequence, output_sequence) = load_cursors(transaction, intent.session_id)?;
    let calls = response
        .content
        .iter()
        .filter_map(|content| match content {
            AssistantContent::ToolCall { call } => Some(call.clone()),
            AssistantContent::Text { .. } | AssistantContent::Reasoning { .. } => None,
        })
        .collect();
    let output = response
        .content
        .iter()
        .filter_map(|content| match content {
            AssistantContent::Text { text, .. } => Some(text.as_str()),
            AssistantContent::Reasoning { .. } | AssistantContent::ToolCall { .. } => None,
        })
        .collect();
    let usage = response.usage;
    let stop_reason = response.stop_reason;
    let message = Message::Assistant {
        content: response.content,
        stop_reason,
        usage,
        metadata: response.metadata,
    };
    transaction
        .execute(
            "INSERT INTO conversation_entries (
                entry_id, session_id, operation_id, sequence, message_json
             ) VALUES (?1, ?2, ?3, ?4, ?5)",
            params![
                intent.assistant_entry_id.to_string(),
                intent.session_id.to_string(),
                intent.operation_id.to_string(),
                entry_sequence,
                serde_json::to_string(&message).map_err(json_error)?,
            ],
        )
        .map_err(sqlite_error)?;
    let changed = transaction
        .execute(
            "UPDATE model_attempts
             SET status = 'completed', request_json = NULL, usage_json = ?3
             WHERE effect_id = ?1 AND settlement_token = ?2 AND status = 'pending'",
            params![
                intent.effect_id.to_string(),
                intent.settlement_token.to_string(),
                serde_json::to_string(&usage).map_err(json_error)?,
            ],
        )
        .map_err(sqlite_error)?;
    if changed != 1 {
        return Err(HarnessError::Corrupt(
            "model attempt settlement compare-and-set failed".to_owned(),
        ));
    }
    Ok(CommittedResponse {
        calls,
        output,
        stop_reason,
        entry_sequence,
        output_sequence,
    })
}

fn classify_committed_response(
    transaction: &rusqlite::Transaction<'_>,
    intent: &ModelIntent,
    old_state_json: &str,
    response: CommittedResponse,
) -> Result<Settlement, HarnessError> {
    if response.calls.is_empty() {
        return finish_model_operation(transaction, intent, old_state_json, response);
    }
    if response.stop_reason == StopReason::Length {
        return continue_after_truncated_calls(transaction, intent, old_state_json, &response);
    }
    continue_with_tool_plan(transaction, intent, old_state_json, &response)
}

fn finish_model_operation(
    transaction: &rusqlite::Transaction<'_>,
    intent: &ModelIntent,
    old_state_json: &str,
    response: CommittedResponse,
) -> Result<Settlement, HarnessError> {
    let outcome = OperationOutcome::Completed {
        output: response.output,
        stop_reason: response.stop_reason,
        usage: aggregate_usage(transaction, intent.operation_id)?,
    };
    insert_output(transaction, intent, response.output_sequence, &outcome)?;
    let state = StoredState::from_state(StoredOperationState::Completed);
    update_state(
        transaction,
        intent.operation_id,
        old_state_json,
        &serde_json::to_string(&state).map_err(json_error)?,
    )?;
    finish_active_operation(transaction, intent.session_id, intent.operation_id, 1)?;
    Ok(Settlement::Applied(outcome))
}

fn continue_after_truncated_calls(
    transaction: &rusqlite::Transaction<'_>,
    intent: &ModelIntent,
    old_state_json: &str,
    response: &CommittedResponse,
) -> Result<Settlement, HarnessError> {
    let entry_count = response
        .calls
        .len()
        .checked_add(1)
        .ok_or_else(|| HarnessError::Corrupt("entry-count increment overflowed".to_owned()))?;
    let inserted_entries = i64::try_from(entry_count)
        .map_err(|_| HarnessError::Corrupt("entry-count increment exceeds i64".to_owned()))?;
    insert_truncated_results(
        transaction,
        intent,
        response
            .entry_sequence
            .checked_add(1)
            .ok_or_else(|| HarnessError::Corrupt("entry cursor overflowed".to_owned()))?,
        &response.calls,
    )?;
    if intent.progress.model_attempts >= intent.progress.runtime.max_model_attempts {
        let outcome = OperationOutcome::Failed {
            message: MODEL_ATTEMPT_LIMIT_AFTER_TOOL_RESULTS.to_owned(),
        };
        insert_output(transaction, intent, response.output_sequence, &outcome)?;
        let state = StoredState::from_state(StoredOperationState::Failed {
            kind: crate::state::FailureKind::General,
        });
        update_state(
            transaction,
            intent.operation_id,
            old_state_json,
            &serde_json::to_string(&state).map_err(json_error)?,
        )?;
        finish_active_operation(
            transaction,
            intent.session_id,
            intent.operation_id,
            inserted_entries,
        )?;
        return Ok(Settlement::Applied(outcome));
    }
    let state = StoredState::from_state(StoredOperationState::NeedModel {
        progress: intent.progress.clone(),
    });
    update_state(
        transaction,
        intent.operation_id,
        old_state_json,
        &serde_json::to_string(&state).map_err(json_error)?,
    )?;
    advance_entry_cursor(
        transaction,
        intent.session_id,
        intent.operation_id,
        inserted_entries,
    )?;
    Ok(Settlement::Continue(state))
}

fn continue_with_tool_plan(
    transaction: &rusqlite::Transaction<'_>,
    intent: &ModelIntent,
    old_state_json: &str,
    response: &CommittedResponse,
) -> Result<Settlement, HarnessError> {
    let batch_id = Uuid::new_v4();
    insert_tool_plan(transaction, intent, batch_id, &response.calls)?;
    let state = StoredState::from_state(StoredOperationState::NeedTool {
        progress: intent.progress.clone(),
        batch: ToolBatch {
            batch_id,
            next_index: 0,
            call_count: u32::try_from(response.calls.len())
                .map_err(|_| HarnessError::Corrupt("tool-call count exceeds u32".to_owned()))?,
        },
    });
    update_state(
        transaction,
        intent.operation_id,
        old_state_json,
        &serde_json::to_string(&state).map_err(json_error)?,
    )?;
    advance_entry_cursor(transaction, intent.session_id, intent.operation_id, 1)?;
    Ok(Settlement::Continue(state))
}

fn insert_tool_plan(
    transaction: &rusqlite::Transaction<'_>,
    intent: &ModelIntent,
    batch_id: Uuid,
    calls: &[ToolCall],
) -> Result<(), HarnessError> {
    for (index, call) in calls.iter().enumerate() {
        transaction
            .execute(
                "INSERT INTO tool_calls (
                    operation_id, batch_id, source_index, result_entry_id, call_json,
                    status, recovery, effect_id, settlement_token
                 ) VALUES (?1, ?2, ?3, ?4, ?5, 'planned', NULL, NULL, NULL)",
                params![
                    intent.operation_id.to_string(),
                    batch_id.to_string(),
                    i64::try_from(index).map_err(|_| {
                        HarnessError::Corrupt("tool-call index exceeds i64".to_owned())
                    })?,
                    Uuid::new_v4().to_string(),
                    serde_json::to_string(call).map_err(json_error)?,
                ],
            )
            .map_err(sqlite_error)?;
    }
    Ok(())
}

fn insert_truncated_results(
    transaction: &rusqlite::Transaction<'_>,
    intent: &ModelIntent,
    first_sequence: i64,
    calls: &[ToolCall],
) -> Result<(), HarnessError> {
    for (index, call) in calls.iter().enumerate() {
        let sequence = first_sequence
            .checked_add(
                i64::try_from(index)
                    .map_err(|_| HarnessError::Corrupt("tool-call index exceeds i64".to_owned()))?,
            )
            .ok_or_else(|| HarnessError::Corrupt("entry cursor overflowed".to_owned()))?;
        let result = ToolResult {
            call_id: call.id.clone(),
            name: call.name.clone(),
            content: vec![ContentBlock::text(
                "Tool call was not executed because the model response reached its token limit.",
            )],
            details: None,
            is_error: true,
        };
        transaction
            .execute(
                "INSERT INTO conversation_entries (
                    entry_id, session_id, operation_id, sequence, message_json
                 ) VALUES (?1, ?2, ?3, ?4, ?5)",
                params![
                    Uuid::new_v4().to_string(),
                    intent.session_id.to_string(),
                    intent.operation_id.to_string(),
                    sequence,
                    serde_json::to_string(&Message::Tool { result }).map_err(json_error)?,
                ],
            )
            .map_err(sqlite_error)?;
    }
    Ok(())
}

fn aggregate_usage(
    transaction: &rusqlite::Transaction<'_>,
    operation_id: crate::OperationId,
) -> Result<Option<TokenUsage>, HarnessError> {
    let mut statement = transaction
        .prepare(
            "SELECT status, usage_json FROM model_attempts
             WHERE operation_id = ?1 ORDER BY attempt_number",
        )
        .map_err(sqlite_error)?;
    let rows = statement
        .query_map([operation_id.to_string()], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, Option<String>>(1)?))
        })
        .map_err(sqlite_error)?;
    let mut total = TokenUsage::default();
    for row in rows {
        let (status, usage_json) = row.map_err(sqlite_error)?;
        if status != "completed" {
            return Ok(None);
        }
        let Some(usage) = usage_json
            .map(|value| serde_json::from_str::<Option<TokenUsage>>(&value).map_err(json_error))
            .transpose()?
            .flatten()
        else {
            return Ok(None);
        };
        add_token_usage(&mut total, usage)?;
    }
    Ok(Some(total))
}
