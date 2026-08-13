use renoa_agent::{Message, ModelRequest};
use rusqlite::{OptionalExtension, Transaction, TransactionBehavior, params};
use uuid::Uuid;

use crate::{
    HarnessError, OperationId, OperationRequest, SessionId, SessionRunLease,
    drive::{ActiveOperation, ModelIntent, ModelStart},
    schema::{json_error, sqlite_error},
    state::{FrozenRuntime, OperationProgress, StoredOperationState, StoredState},
    store::{Store, blocking_transition, parse_operation_id},
    store_support::{
        cancellation_requested, finish_cancelled_operation, load_messages, parse_session_id,
        parse_state, update_state,
    },
};

impl Store {
    pub(crate) async fn activate(
        &self,
        lease: &std::sync::Arc<SessionRunLease>,
        session_id: SessionId,
        runtime: FrozenRuntime,
    ) -> Result<Option<ActiveOperation>, HarnessError> {
        let database = self.database();
        let lease = std::sync::Arc::clone(lease);
        blocking_transition(lease, move || {
            let mut connection = database.connection()?;
            let transaction = connection
                .transaction_with_behavior(TransactionBehavior::Immediate)
                .map_err(sqlite_error)?;
            let session = transaction
                .query_row(
                    "SELECT active_operation_id, next_entry_sequence
                     FROM sessions WHERE session_id = ?1",
                    [session_id.to_string()],
                    |row| Ok((row.get::<_, Option<String>>(0)?, row.get::<_, i64>(1)?)),
                )
                .optional()
                .map_err(sqlite_error)?
                .ok_or(HarnessError::SessionNotFound(session_id))?;
            if let Some(operation_id) = session.0 {
                let operation_id = parse_operation_id(&operation_id)?;
                let state = load_state(&transaction, operation_id)?;
                transaction.commit().map_err(sqlite_error)?;
                return Ok(Some(ActiveOperation {
                    operation_id,
                    state,
                    #[cfg(test)]
                    newly_activated: false,
                }));
            }

            let Some((operation_id, request_json, old_state_json)) =
                load_next_queued(&transaction, session_id)?
            else {
                transaction.commit().map_err(sqlite_error)?;
                return Ok(None);
            };
            let operation_id = parse_operation_id(&operation_id)?;
            let request: OperationRequest =
                serde_json::from_str(&request_json).map_err(json_error)?;
            let state = StoredState::from_state(StoredOperationState::NeedModel {
                progress: OperationProgress {
                    runtime,
                    model_attempts: 0,
                    compaction_attempts: 0,
                    force_compaction: false,
                },
            });
            let state_json = serde_json::to_string(&state).map_err(json_error)?;
            transaction
                .execute(
                    "INSERT INTO conversation_entries (
                        entry_id, session_id, operation_id, sequence, message_json
                     ) VALUES (?1, ?2, ?3, ?4, ?5)",
                    params![
                        Uuid::new_v4().to_string(),
                        session_id.to_string(),
                        operation_id.to_string(),
                        session.1,
                        serde_json::to_string(&request.into_message()).map_err(json_error)?,
                    ],
                )
                .map_err(sqlite_error)?;
            update_state(&transaction, operation_id, &old_state_json, &state_json)?;
            let changed = transaction
                .execute(
                    "UPDATE sessions
                     SET active_operation_id = ?2, next_entry_sequence = next_entry_sequence + 1
                     WHERE session_id = ?1 AND active_operation_id IS NULL",
                    params![session_id.to_string(), operation_id.to_string()],
                )
                .map_err(sqlite_error)?;
            if changed != 1 {
                return Err(HarnessError::Corrupt(
                    "session activation compare-and-set failed".to_owned(),
                ));
            }
            transaction.commit().map_err(sqlite_error)?;
            Ok(Some(ActiveOperation {
                operation_id,
                state,
                #[cfg(test)]
                newly_activated: true,
            }))
        })
        .await
    }

    pub(crate) async fn begin_model_attempt(
        &self,
        lease: &std::sync::Arc<SessionRunLease>,
        operation_id: OperationId,
        projected_request: Option<ModelRequest>,
    ) -> Result<ModelStart, HarnessError> {
        let database = self.database();
        let lease = std::sync::Arc::clone(lease);
        blocking_transition(lease, move || {
            let mut connection = database.connection()?;
            let transaction = connection
                .transaction_with_behavior(TransactionBehavior::Immediate)
                .map_err(sqlite_error)?;
            let (session_id, old_state_json) = transaction
                .query_row(
                    "SELECT session_id, state_json FROM operations WHERE operation_id = ?1",
                    [operation_id.to_string()],
                    |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
                )
                .optional()
                .map_err(sqlite_error)?
                .ok_or_else(|| HarnessError::Corrupt("active operation is missing".to_owned()))?;
            let session_id = parse_session_id(&session_id)?;
            let old_state = parse_state(&old_state_json)?;
            let StoredOperationState::NeedModel { progress } = old_state.state() else {
                return Err(HarnessError::Corrupt(
                    "model intent requires NeedModel state".to_owned(),
                ));
            };
            if cancellation_requested(&transaction, operation_id)? {
                let outcome = cancel_before_model_attempt(
                    &transaction,
                    session_id,
                    operation_id,
                    &old_state_json,
                )?;
                transaction.commit().map_err(sqlite_error)?;
                return Ok(ModelStart::Finished(outcome));
            }
            let request = match projected_request {
                Some(request) => request,
                None => build_model_request(&transaction, session_id, progress)?,
            };
            let effect_id = Uuid::new_v4();
            let settlement_token = Uuid::new_v4();
            let assistant_entry_id = Uuid::new_v4();
            let output_id = Uuid::new_v4();
            let attempt_count = progress.model_attempts.checked_add(1).ok_or_else(|| {
                HarnessError::Corrupt("model attempt counter overflowed".to_owned())
            })?;
            if attempt_count > progress.runtime.max_model_attempts {
                return Err(HarnessError::Corrupt(
                    "model attempt limit was exceeded before dispatch".to_owned(),
                ));
            }
            let progress = OperationProgress {
                runtime: progress.runtime.clone(),
                model_attempts: attempt_count,
                compaction_attempts: progress.compaction_attempts,
                force_compaction: progress.force_compaction,
            };
            let state = StoredState::from_state(StoredOperationState::ModelPending {
                progress: progress.clone(),
                effect_id,
                settlement_token,
                assistant_entry_id,
                output_id,
            });
            transaction
                .execute(
                    "INSERT INTO model_attempts (
                        effect_id, operation_id, attempt_number, settlement_token, status,
                        request_json, usage_json, error
                     ) VALUES (?1, ?2, ?3, ?4, 'pending', ?5, NULL, NULL)",
                    params![
                        effect_id.to_string(),
                        operation_id.to_string(),
                        i64::from(progress.model_attempts),
                        settlement_token.to_string(),
                        serde_json::to_string(&request).map_err(json_error)?,
                    ],
                )
                .map_err(sqlite_error)?;
            update_state(
                &transaction,
                operation_id,
                &old_state_json,
                &serde_json::to_string(&state).map_err(json_error)?,
            )?;
            transaction.commit().map_err(sqlite_error)?;
            Ok(ModelStart::Invoke(Box::new(ModelIntent {
                session_id,
                operation_id,
                effect_id,
                settlement_token,
                assistant_entry_id,
                output_id,
                progress,
                request,
            })))
        })
        .await
    }

    pub(crate) async fn load_model_messages(
        &self,
        lease: &std::sync::Arc<SessionRunLease>,
        operation_id: OperationId,
    ) -> Result<Vec<Message>, HarnessError> {
        let database = self.database();
        let lease = std::sync::Arc::clone(lease);
        blocking_transition(lease, move || {
            let mut connection = database.connection()?;
            let transaction = connection
                .transaction_with_behavior(TransactionBehavior::Deferred)
                .map_err(sqlite_error)?;
            let (session_id, state_json) = transaction
                .query_row(
                    "SELECT session_id, state_json FROM operations WHERE operation_id = ?1",
                    [operation_id.to_string()],
                    |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
                )
                .optional()
                .map_err(sqlite_error)?
                .ok_or_else(|| HarnessError::Corrupt("active operation is missing".to_owned()))?;
            if !matches!(
                parse_state(&state_json)?.state(),
                StoredOperationState::NeedModel { .. }
            ) {
                return Err(HarnessError::Corrupt(
                    "context projection requires NeedModel state".to_owned(),
                ));
            }
            let session_id = parse_session_id(&session_id)?;
            let messages =
                crate::checkpoint::load_context_view(&transaction, session_id, operation_id)?;
            transaction.commit().map_err(sqlite_error)?;
            Ok(messages)
        })
        .await
    }
}

fn cancel_before_model_attempt(
    transaction: &Transaction<'_>,
    session_id: SessionId,
    operation_id: OperationId,
    old_state_json: &str,
) -> Result<crate::OperationOutcome, HarnessError> {
    finish_cancelled_operation(
        transaction,
        session_id,
        operation_id,
        Uuid::new_v4(),
        old_state_json,
        0,
    )
}

fn build_model_request(
    transaction: &Transaction<'_>,
    session_id: SessionId,
    progress: &OperationProgress,
) -> Result<ModelRequest, HarnessError> {
    Ok(ModelRequest {
        system_prompt: progress.runtime.system_prompt.clone(),
        messages: load_messages(transaction, session_id)?,
        tools: progress
            .runtime
            .tools
            .iter()
            .map(|tool| tool.spec.clone())
            .collect(),
    })
}

fn load_state(
    transaction: &Transaction<'_>,
    operation_id: OperationId,
) -> Result<StoredState, HarnessError> {
    let state_json = transaction
        .query_row(
            "SELECT state_json FROM operations WHERE operation_id = ?1",
            [operation_id.to_string()],
            |row| row.get::<_, String>(0),
        )
        .optional()
        .map_err(sqlite_error)?
        .ok_or_else(|| HarnessError::Corrupt("active operation is missing".to_owned()))?;
    parse_state(&state_json)
}

fn load_next_queued(
    transaction: &Transaction<'_>,
    session_id: SessionId,
) -> Result<Option<(String, String, String)>, HarnessError> {
    let mut statement = transaction
        .prepare(
            "SELECT operation_id, request_json, state_json
             FROM operations WHERE session_id = ?1 ORDER BY position",
        )
        .map_err(sqlite_error)?;
    let rows = statement
        .query_map([session_id.to_string()], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
            ))
        })
        .map_err(sqlite_error)?;
    for row in rows {
        let row = row.map_err(sqlite_error)?;
        match parse_state(&row.2)?.state() {
            StoredOperationState::Queued => return Ok(Some(row)),
            StoredOperationState::Completed | StoredOperationState::Failed { .. } => {}
            StoredOperationState::NeedModel { .. }
            | StoredOperationState::ModelPending { .. }
            | StoredOperationState::CompactionPending { .. }
            | StoredOperationState::NeedTool { .. }
            | StoredOperationState::ToolPending { .. }
            | StoredOperationState::ToolOutcomeUnknown { .. } => {
                return Err(HarnessError::Corrupt(
                    "inactive session contains a non-terminal active operation".to_owned(),
                ));
            }
        }
    }
    Ok(None)
}
