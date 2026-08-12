use renoa_agent::{AssistantContent, Message, ModelResponse, TokenUsage};
use rusqlite::{TransactionBehavior, params};

use crate::{
    HarnessError, OperationOutcome, SessionRunLease,
    drive::{ModelIntent, Settlement},
    schema::{json_error, sqlite_error},
    state::{StoredOperationState, StoredState},
    store::{Store, blocking_transition},
    store_support::{
        current_pending_state, finish_failed_operation, finish_session, insert_output,
        load_cursors, update_state,
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
            let Some(old_state_json) = current_pending_state(&transaction, &intent)? else {
                return Ok(Settlement::Stale);
            };
            let (entry_sequence, output_sequence) = load_cursors(&transaction, intent.session_id)?;
            let output = response
                .content
                .iter()
                .filter_map(|content| match content {
                    AssistantContent::Text { text, .. } => Some(text.as_str()),
                    AssistantContent::Reasoning { .. } | AssistantContent::ToolCall { .. } => None,
                })
                .collect::<String>();
            let outcome = OperationOutcome::Completed {
                output,
                stop_reason: response.stop_reason,
                usage: response.usage,
            };
            let message = Message::Assistant {
                content: response.content,
                stop_reason: response.stop_reason,
                usage: response.usage,
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
            insert_output(&transaction, &intent, output_sequence, &outcome)?;
            let changed = transaction
                .execute(
                    "UPDATE model_attempts
                     SET status = 'completed', request_json = NULL, usage_json = ?3
                     WHERE effect_id = ?1 AND settlement_token = ?2 AND status = 'pending'",
                    params![
                        intent.effect_id.to_string(),
                        intent.settlement_token.to_string(),
                        serde_json::to_string(&response.usage).map_err(json_error)?,
                    ],
                )
                .map_err(sqlite_error)?;
            if changed != 1 {
                return Err(HarnessError::Corrupt(
                    "model attempt settlement compare-and-set failed".to_owned(),
                ));
            }
            let state = StoredState::from_state(StoredOperationState::Completed);
            update_state(
                &transaction,
                intent.operation_id,
                &old_state_json,
                &serde_json::to_string(&state).map_err(json_error)?,
            )?;
            finish_session(&transaction, &intent, true)?;
            transaction.commit().map_err(sqlite_error)?;
            Ok(Settlement::Applied(outcome))
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
