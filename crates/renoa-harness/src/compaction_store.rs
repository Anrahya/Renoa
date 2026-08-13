use renoa_agent::TokenUsage;
use rusqlite::{Transaction, TransactionBehavior, params};
use uuid::Uuid;

use crate::{
    HarnessError, OperationId, SessionRunLease,
    checkpoint::{load_active_checkpoint, load_entries_after, load_operation_user_anchor},
    compaction::{
        CompactionAttempt, CompactionIntent, CompactionPlan, CompactionRecovery, CompactionSource,
        CompactionStart,
    },
    compaction_store_support::{
        activate_checkpoint, complete_attempt, current_pending_state, insert_attempt,
        load_operation, load_pending_intent, mark_attempt_unknown, next_attempt, validate_plan,
    },
    schema::{json_error, sqlite_error},
    state::{OperationProgress, StoredOperationState, StoredState},
    store::{Store, blocking_transition},
    store_support::{
        cancellation_requested, finish_cancelled_operation, finish_operation_failure, parse_state,
        update_state,
    },
};

enum AttemptCompletion {
    Completed(Option<TokenUsage>),
    OutcomeUnknown,
}

impl Store {
    pub(crate) async fn load_compaction_source(
        &self,
        lease: &std::sync::Arc<SessionRunLease>,
        operation_id: OperationId,
    ) -> Result<CompactionSource, HarnessError> {
        let database = self.database();
        let lease = std::sync::Arc::clone(lease);
        blocking_transition(lease, move || {
            let mut connection = database.connection()?;
            let transaction = connection
                .transaction_with_behavior(TransactionBehavior::Deferred)
                .map_err(sqlite_error)?;
            let (session_id, state_json) = load_operation(&transaction, operation_id)?;
            let state = parse_state(&state_json)?;
            let StoredOperationState::NeedModel { progress } = state.state() else {
                return Err(HarnessError::Corrupt(
                    "compaction planning requires NeedModel state".to_owned(),
                ));
            };
            let checkpoint = load_active_checkpoint(&transaction, session_id)?;
            let covered = checkpoint
                .as_ref()
                .map(|checkpoint| checkpoint.covered_through_sequence);
            let entries = load_entries_after(&transaction, session_id, covered)?;
            let source = CompactionSource {
                progress: progress.clone(),
                checkpoint,
                active_user_anchor: load_operation_user_anchor(
                    &transaction,
                    session_id,
                    operation_id,
                )?,
                entries,
            };
            transaction.commit().map_err(sqlite_error)?;
            Ok(source)
        })
        .await
    }

    pub(crate) async fn begin_compaction(
        &self,
        lease: &std::sync::Arc<SessionRunLease>,
        operation_id: OperationId,
        plan: CompactionPlan,
    ) -> Result<CompactionStart, HarnessError> {
        let database = self.database();
        let lease = std::sync::Arc::clone(lease);
        blocking_transition(lease, move || {
            let mut connection = database.connection()?;
            let transaction = connection
                .transaction_with_behavior(TransactionBehavior::Immediate)
                .map_err(sqlite_error)?;
            let (session_id, old_state_json) = load_operation(&transaction, operation_id)?;
            let state = parse_state(&old_state_json)?;
            let StoredOperationState::NeedModel { progress } = state.state() else {
                return Err(HarnessError::Corrupt(
                    "compaction intent requires NeedModel state".to_owned(),
                ));
            };
            let output_id = Uuid::new_v4();
            if cancellation_requested(&transaction, operation_id)? {
                let outcome = finish_cancelled_operation(
                    &transaction,
                    session_id,
                    operation_id,
                    output_id,
                    &old_state_json,
                    0,
                )?;
                transaction.commit().map_err(sqlite_error)?;
                return Ok(CompactionStart::Finished(outcome));
            }
            validate_plan(&transaction, session_id, &plan)?;
            let next_attempt = next_attempt(progress)?;
            if progress.runtime.compaction.is_none() {
                return Err(HarnessError::Corrupt(
                    "compaction began without frozen limits".to_owned(),
                ));
            }
            let effect_id = Uuid::new_v4();
            let settlement_token = Uuid::new_v4();
            let progress = OperationProgress {
                runtime: progress.runtime.clone(),
                model_attempts: progress.model_attempts,
                compaction_attempts: next_attempt,
                force_compaction: progress.force_compaction,
            };
            insert_attempt(
                &transaction,
                operation_id,
                effect_id,
                settlement_token,
                next_attempt,
                &plan,
            )?;
            let next_state = StoredState::from_state(StoredOperationState::CompactionPending {
                progress: progress.clone(),
                effect_id,
                settlement_token,
                checkpoint_id: plan.checkpoint_id,
                output_id,
            });
            update_state(
                &transaction,
                operation_id,
                &old_state_json,
                &serde_json::to_string(&next_state).map_err(json_error)?,
            )?;
            transaction.commit().map_err(sqlite_error)?;
            Ok(CompactionStart::Invoke(Box::new(CompactionIntent {
                session_id,
                operation_id,
                effect_id,
                settlement_token,
                output_id,
                progress,
                plan,
            })))
        })
        .await
    }

    pub(crate) async fn recover_compaction(
        &self,
        lease: &std::sync::Arc<SessionRunLease>,
        operation_id: OperationId,
    ) -> Result<CompactionRecovery, HarnessError> {
        let database = self.database();
        let lease = std::sync::Arc::clone(lease);
        blocking_transition(lease, move || {
            let mut connection = database.connection()?;
            let transaction = connection
                .transaction_with_behavior(TransactionBehavior::Immediate)
                .map_err(sqlite_error)?;
            let (intent, old_state_json) = load_pending_intent(&transaction, operation_id)?;
            mark_attempt_unknown(&transaction, &intent)?;
            if cancellation_requested(&transaction, operation_id)? {
                let outcome = finish_cancelled_operation(
                    &transaction,
                    intent.session_id,
                    operation_id,
                    intent.output_id,
                    &old_state_json,
                    0,
                )?;
                transaction.commit().map_err(sqlite_error)?;
                return Ok(CompactionRecovery::Finished(outcome));
            }
            let result = retry_or_fail(
                &transaction,
                intent,
                &old_state_json,
                "compaction outcome was unknown after process loss".to_owned(),
            )?;
            transaction.commit().map_err(sqlite_error)?;
            match result {
                CompactionAttempt::Retry(intent) => Ok(CompactionRecovery::Retry(intent)),
                CompactionAttempt::Finished(outcome) => Ok(CompactionRecovery::Finished(outcome)),
                CompactionAttempt::Continue(_) | CompactionAttempt::Stale => {
                    Err(HarnessError::Corrupt(
                        "compaction recovery produced an invalid state".to_owned(),
                    ))
                }
            }
        })
        .await
    }

    pub(crate) async fn settle_compaction(
        &self,
        lease: &std::sync::Arc<SessionRunLease>,
        intent: CompactionIntent,
        summary: String,
        usage: Option<TokenUsage>,
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
            complete_attempt(&transaction, &intent, usage)?;
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
                return Ok(CompactionAttempt::Finished(outcome));
            }
            transaction
                .execute(
                    "INSERT INTO context_checkpoints (
                        checkpoint_id, session_id, previous_checkpoint_id,
                        covered_through_sequence, summary
                     ) VALUES (?1, ?2, ?3, ?4, ?5)",
                    params![
                        intent.plan.checkpoint_id.to_string(),
                        intent.session_id.to_string(),
                        intent.plan.previous_checkpoint_id.map(|id| id.to_string()),
                        i64::try_from(intent.plan.covered_through_sequence).map_err(|_| {
                            HarnessError::Corrupt("checkpoint sequence exceeds i64".to_owned())
                        })?,
                        summary,
                    ],
                )
                .map_err(sqlite_error)?;
            activate_checkpoint(&transaction, &intent)?;
            let next_state = StoredState::from_state(StoredOperationState::NeedModel {
                progress: OperationProgress {
                    force_compaction: false,
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
            Ok(CompactionAttempt::Continue(next_state))
        })
        .await
    }

    pub(crate) async fn reject_compaction(
        &self,
        lease: &std::sync::Arc<SessionRunLease>,
        intent: CompactionIntent,
        usage: Option<TokenUsage>,
        message: String,
    ) -> Result<CompactionAttempt, HarnessError> {
        self.finish_unsuccessful_compaction(
            lease,
            intent,
            AttemptCompletion::Completed(usage),
            message,
        )
        .await
    }

    pub(crate) async fn record_compaction_uncertainty(
        &self,
        lease: &std::sync::Arc<SessionRunLease>,
        intent: CompactionIntent,
        message: String,
    ) -> Result<CompactionAttempt, HarnessError> {
        self.finish_unsuccessful_compaction(
            lease,
            intent,
            AttemptCompletion::OutcomeUnknown,
            message,
        )
        .await
    }

    async fn finish_unsuccessful_compaction(
        &self,
        lease: &std::sync::Arc<SessionRunLease>,
        intent: CompactionIntent,
        completion: AttemptCompletion,
        message: String,
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
            match completion {
                AttemptCompletion::Completed(usage) => {
                    complete_attempt(&transaction, &intent, usage)?;
                }
                AttemptCompletion::OutcomeUnknown => {
                    mark_attempt_unknown(&transaction, &intent)?;
                }
            }
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
                return Ok(CompactionAttempt::Finished(outcome));
            }
            let result = retry_or_fail(&transaction, intent, &old_state_json, message)?;
            transaction.commit().map_err(sqlite_error)?;
            Ok(result)
        })
        .await
    }
}

fn retry_or_fail(
    transaction: &Transaction<'_>,
    intent: CompactionIntent,
    old_state_json: &str,
    message: String,
) -> Result<CompactionAttempt, HarnessError> {
    let frozen = intent.progress.runtime.compaction.ok_or_else(|| {
        HarnessError::Corrupt("pending compaction has no frozen limits".to_owned())
    })?;
    if checkpoint_attempt_count(transaction, &intent)? >= frozen.max_attempts {
        return finish_operation_failure(
            transaction,
            intent.session_id,
            intent.operation_id,
            intent.output_id,
            old_state_json,
            message,
        )
        .map(CompactionAttempt::Finished);
    }
    let next_attempt = intent
        .progress
        .compaction_attempts
        .checked_add(1)
        .ok_or_else(|| HarnessError::Corrupt("compaction counter overflowed".to_owned()))?;
    let effect_id = Uuid::new_v4();
    let settlement_token = Uuid::new_v4();
    insert_attempt(
        transaction,
        intent.operation_id,
        effect_id,
        settlement_token,
        next_attempt,
        &intent.plan,
    )?;
    let progress = OperationProgress {
        runtime: intent.progress.runtime.clone(),
        model_attempts: intent.progress.model_attempts,
        compaction_attempts: next_attempt,
        force_compaction: intent.progress.force_compaction,
    };
    let output_id = Uuid::new_v4();
    let next_state = StoredState::from_state(StoredOperationState::CompactionPending {
        progress: progress.clone(),
        effect_id,
        settlement_token,
        checkpoint_id: intent.plan.checkpoint_id,
        output_id,
    });
    update_state(
        transaction,
        intent.operation_id,
        old_state_json,
        &serde_json::to_string(&next_state).map_err(json_error)?,
    )?;
    Ok(CompactionAttempt::Retry(Box::new(CompactionIntent {
        session_id: intent.session_id,
        operation_id: intent.operation_id,
        effect_id,
        settlement_token,
        output_id,
        progress,
        plan: intent.plan,
    })))
}

fn checkpoint_attempt_count(
    transaction: &Transaction<'_>,
    intent: &CompactionIntent,
) -> Result<u32, HarnessError> {
    let count = transaction
        .query_row(
            "SELECT COUNT(*) FROM compaction_attempts
             WHERE operation_id = ?1 AND checkpoint_id = ?2",
            params![
                intent.operation_id.to_string(),
                intent.plan.checkpoint_id.to_string()
            ],
            |row| row.get::<_, i64>(0),
        )
        .map_err(sqlite_error)?;
    u32::try_from(count)
        .map_err(|_| HarnessError::Corrupt("compaction attempt count exceeds u32".to_owned()))
}
