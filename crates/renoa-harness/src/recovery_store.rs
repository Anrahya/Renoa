use renoa_agent::ModelRequest;
use rusqlite::{OptionalExtension, Transaction, TransactionBehavior, params};

use crate::{
    HarnessError, OperationId, SessionRunLease,
    drive::{ModelIntent, PendingRecovery, UncertainAttempt},
    schema::{json_error, sqlite_error},
    state::StoredOperationState,
    store::{Store, blocking_transition},
    store_support::{
        current_pending_state, finish_failed_operation, insert_retry_intent, parse_session_id,
        parse_state,
    },
};

impl Store {
    pub(crate) async fn recover_model_attempt(
        &self,
        lease: &std::sync::Arc<SessionRunLease>,
        operation_id: OperationId,
    ) -> Result<PendingRecovery, HarnessError> {
        let database = self.database();
        let lease = std::sync::Arc::clone(lease);
        blocking_transition(lease, move || {
            let mut connection = database.connection()?;
            let transaction = connection
                .transaction_with_behavior(TransactionBehavior::Immediate)
                .map_err(sqlite_error)?;
            let (previous, old_state_json, request_json) =
                load_pending_recovery(&transaction, operation_id)?;
            let changed = transaction
                .execute(
                    "UPDATE model_attempts
                     SET status = 'outcome_unknown', request_json = NULL, error = ?3
                     WHERE effect_id = ?1 AND settlement_token = ?2 AND status = 'pending'",
                    params![
                        previous.effect_id.to_string(),
                        previous.settlement_token.to_string(),
                        "process stopped before the model attempt settled",
                    ],
                )
                .map_err(sqlite_error)?;
            if changed != 1 {
                return Err(HarnessError::Corrupt(
                    "uncertain model attempt compare-and-set failed".to_owned(),
                ));
            }

            let recovery = if previous.attempt_count >= previous.max_model_attempts {
                PendingRecovery::Finished(finish_failed_operation(
                    &transaction,
                    &previous,
                    &old_state_json,
                    "model attempt outcome is unknown after restart and the retry limit is exhausted"
                        .to_owned(),
                )?)
            } else {
                PendingRecovery::Retry(insert_retry_intent(
                    &transaction,
                    &previous,
                    &old_state_json,
                    &request_json,
                )?)
            };
            transaction.commit().map_err(sqlite_error)?;
            Ok(recovery)
        })
        .await
    }

    pub(crate) async fn record_model_uncertainty(
        &self,
        lease: &std::sync::Arc<SessionRunLease>,
        intent: ModelIntent,
        message: String,
    ) -> Result<UncertainAttempt, HarnessError> {
        let database = self.database();
        let lease = std::sync::Arc::clone(lease);
        blocking_transition(lease, move || {
            let mut connection = database.connection()?;
            let transaction = connection
                .transaction_with_behavior(TransactionBehavior::Immediate)
                .map_err(sqlite_error)?;
            let Some(old_state_json) = current_pending_state(&transaction, &intent)? else {
                return Ok(UncertainAttempt::Stale);
            };
            let request_json = if intent.attempt_count < intent.max_model_attempts {
                Some(
                    transaction
                        .query_row(
                            "SELECT request_json FROM model_attempts WHERE effect_id = ?1",
                            [intent.effect_id.to_string()],
                            |row| row.get::<_, Option<String>>(0),
                        )
                        .map_err(sqlite_error)?
                        .ok_or_else(|| {
                            HarnessError::Corrupt("pending model request is missing".to_owned())
                        })?,
                )
            } else {
                None
            };
            let changed = transaction
                .execute(
                    "UPDATE model_attempts
                     SET status = 'outcome_unknown', request_json = NULL, error = ?3
                     WHERE effect_id = ?1 AND settlement_token = ?2 AND status = 'pending'",
                    params![
                        intent.effect_id.to_string(),
                        intent.settlement_token.to_string(),
                        &message,
                    ],
                )
                .map_err(sqlite_error)?;
            if changed != 1 {
                return Err(HarnessError::Corrupt(
                    "uncertain model attempt compare-and-set failed".to_owned(),
                ));
            }
            let result = if intent.attempt_count < intent.max_model_attempts {
                UncertainAttempt::Retry(insert_retry_intent(
                    &transaction,
                    &intent,
                    &old_state_json,
                    request_json.as_deref().ok_or_else(|| {
                        HarnessError::Corrupt("pending model request is missing".to_owned())
                    })?,
                )?)
            } else {
                UncertainAttempt::Finished(finish_failed_operation(
                    &transaction,
                    &intent,
                    &old_state_json,
                    message,
                )?)
            };
            transaction.commit().map_err(sqlite_error)?;
            Ok(result)
        })
        .await
    }
}

fn load_pending_recovery(
    transaction: &Transaction<'_>,
    operation_id: OperationId,
) -> Result<(ModelIntent, String, String), HarnessError> {
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
    let StoredOperationState::ModelPending {
        runtime_revision,
        max_model_attempts,
        attempt_count,
        effect_id,
        settlement_token,
        assistant_entry_id,
        output_id,
    } = state.state()
    else {
        return Err(HarnessError::Corrupt(
            "model recovery requires ModelPending state".to_owned(),
        ));
    };
    let (request_json, stored_token, status) = transaction
        .query_row(
            "SELECT request_json, settlement_token, status
             FROM model_attempts WHERE effect_id = ?1 AND operation_id = ?2",
            params![effect_id.to_string(), operation_id.to_string()],
            |row| {
                Ok((
                    row.get::<_, Option<String>>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                ))
            },
        )
        .optional()
        .map_err(sqlite_error)?
        .ok_or_else(|| HarnessError::Corrupt("pending model attempt is missing".to_owned()))?;
    let request_json = request_json
        .ok_or_else(|| HarnessError::Corrupt("pending model request is missing".to_owned()))?;
    if status != "pending" || stored_token != settlement_token.to_string() {
        return Err(HarnessError::Corrupt(
            "pending model attempt does not match operation state".to_owned(),
        ));
    }
    let intent = ModelIntent {
        session_id: parse_session_id(&session_id)?,
        operation_id,
        effect_id: *effect_id,
        settlement_token: *settlement_token,
        assistant_entry_id: *assistant_entry_id,
        output_id: *output_id,
        runtime_revision: runtime_revision.clone(),
        max_model_attempts: *max_model_attempts,
        attempt_count: *attempt_count,
        request: serde_json::from_str::<ModelRequest>(&request_json).map_err(json_error)?,
    };
    Ok((intent, state_json, request_json))
}
