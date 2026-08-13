use rusqlite::TransactionBehavior;
use uuid::Uuid;

use crate::{
    HarnessError, OperationId, OperationOutcome, SessionRunLease,
    compaction::{CompactionAttempt, CompactionIntent},
    compaction_store_support::{complete_attempt, current_pending_state, load_operation},
    drive::{ModelIntent, Settlement},
    schema::{json_error, sqlite_error},
    state::{OperationProgress, StoredOperationState, StoredState},
    store::{Store, blocking_transition},
    store_support::{
        cancellation_requested, complete_model_attempt,
        current_pending_state as current_model_state, finish_cancelled_operation,
        finish_failed_operation, finish_operation_failure, parse_state, update_state,
    },
};

impl Store {
    pub(crate) async fn finish_context_capacity_failure(
        &self,
        lease: &std::sync::Arc<SessionRunLease>,
        operation_id: OperationId,
        estimated_tokens: u64,
        dispatch_limit_tokens: u64,
    ) -> Result<OperationOutcome, HarnessError> {
        let database = self.database();
        let lease = std::sync::Arc::clone(lease);
        blocking_transition(lease, move || {
            let mut connection = database.connection()?;
            let transaction = connection
                .transaction_with_behavior(TransactionBehavior::Immediate)
                .map_err(sqlite_error)?;
            let (session_id, old_state_json) = load_operation(&transaction, operation_id)?;
            let state = parse_state(&old_state_json)?;
            if !matches!(state.state(), StoredOperationState::NeedModel { .. }) {
                return Err(HarnessError::Corrupt(
                    "context capacity failure requires NeedModel state".to_owned(),
                ));
            }
            let output_id = Uuid::new_v4();
            let outcome = if cancellation_requested(&transaction, operation_id)? {
                finish_cancelled_operation(
                    &transaction,
                    session_id,
                    operation_id,
                    output_id,
                    &old_state_json,
                    0,
                )?
            } else {
                finish_operation_failure(
                    &transaction,
                    session_id,
                    operation_id,
                    output_id,
                    &old_state_json,
                    format!(
                        "context cannot be reduced below the provider limit: estimated {estimated_tokens} input tokens, dispatch limit {dispatch_limit_tokens}"
                    ),
                )?
            };
            transaction.commit().map_err(sqlite_error)?;
            Ok(outcome)
        })
        .await
    }

    pub(crate) async fn record_context_overflow(
        &self,
        lease: &std::sync::Arc<SessionRunLease>,
        intent: ModelIntent,
        provider_message: String,
    ) -> Result<Settlement, HarnessError> {
        let database = self.database();
        let lease = std::sync::Arc::clone(lease);
        blocking_transition(lease, move || {
            let mut connection = database.connection()?;
            let transaction = connection
                .transaction_with_behavior(TransactionBehavior::Immediate)
                .map_err(sqlite_error)?;
            let Some(old_state_json) = current_model_state(&transaction, &intent)? else {
                return Ok(Settlement::Stale);
            };
            let message = format!("provider rejected model context: {provider_message}");
            complete_model_attempt(
                &transaction,
                &intent,
                None,
                Some(&message),
                "context-window rejection",
            )?;
            if cancellation_requested(&transaction, intent.operation_id)? {
                let outcome = finish_cancelled_operation(
                    &transaction,
                    intent.session_id,
                    intent.operation_id,
                    intent.output_id,
                    &old_state_json,
                    0,
                )?;
                transaction.commit().map_err(sqlite_error)?;
                return Ok(Settlement::Applied(outcome));
            }
            if intent.progress.runtime.compaction.is_none()
                || intent.progress.model_attempts >= intent.progress.runtime.max_model_attempts
            {
                let outcome =
                    finish_failed_operation(&transaction, &intent, &old_state_json, message)?;
                transaction.commit().map_err(sqlite_error)?;
                return Ok(Settlement::Applied(outcome));
            }
            let next_state = StoredState::from_state(StoredOperationState::NeedModel {
                progress: OperationProgress {
                    force_compaction: true,
                    ..intent.progress.clone()
                },
            });
            update_state(
                &transaction,
                intent.operation_id,
                &old_state_json,
                &serde_json::to_string(&next_state).map_err(json_error)?,
            )?;
            transaction.commit().map_err(sqlite_error)?;
            Ok(Settlement::Continue(next_state))
        })
        .await
    }

    pub(crate) async fn fail_compaction_context_overflow(
        &self,
        lease: &std::sync::Arc<SessionRunLease>,
        intent: CompactionIntent,
        provider_message: String,
    ) -> Result<CompactionAttempt, HarnessError> {
        let database = self.database();
        let lease = std::sync::Arc::clone(lease);
        blocking_transition(lease, move || {
            let mut connection = database.connection()?;
            let transaction = connection
                .transaction_with_behavior(TransactionBehavior::Immediate)
                .map_err(sqlite_error)?;
            let Some(old_state_json) = current_pending_state(&transaction, &intent)? else {
                return Ok(CompactionAttempt::Stale);
            };
            complete_attempt(&transaction, &intent, None)?;
            let outcome = if cancellation_requested(&transaction, intent.operation_id)? {
                finish_cancelled_operation(
                    &transaction,
                    intent.session_id,
                    intent.operation_id,
                    intent.output_id,
                    &old_state_json,
                    0,
                )?
            } else {
                finish_operation_failure(
                    &transaction,
                    intent.session_id,
                    intent.operation_id,
                    intent.output_id,
                    &old_state_json,
                    format!("compaction request exceeded provider context: {provider_message}"),
                )?
            };
            transaction.commit().map_err(sqlite_error)?;
            Ok(CompactionAttempt::Finished(outcome))
        })
        .await
    }
}
