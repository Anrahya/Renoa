use rusqlite::{OptionalExtension, TransactionBehavior, params};

use crate::{
    CancellationId, HarnessError, OperationId, SessionId,
    schema::sqlite_error,
    state::StoredOperationState,
    store::{Store, blocking, parse_operation_id},
    store_support::parse_state,
};

impl Store {
    pub(crate) async fn request_cancellation<F>(
        &self,
        session_id: SessionId,
        operation_id: OperationId,
        cancellation_id: CancellationId,
        after_commit: F,
    ) -> Result<(), HarnessError>
    where
        F: FnOnce() -> Result<(), HarnessError> + Send + 'static,
    {
        let database = self.database();
        blocking(move || {
            let mut connection = database.connection()?;
            let transaction = connection
                .transaction_with_behavior(TransactionBehavior::Immediate)
                .map_err(sqlite_error)?;
            let session_key = session_id.to_string();
            let operation_key = operation_id.to_string();
            if let Some((stored_session, stored_operation)) = transaction
                .query_row(
                    "SELECT session_id, operation_id FROM cancellation_requests
                     WHERE cancellation_id = ?1",
                    [cancellation_id.to_string()],
                    |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
                )
                .optional()
                .map_err(sqlite_error)?
            {
                if stored_session == session_key && stored_operation == operation_key {
                    let should_signal = transaction
                        .query_row(
                            "SELECT COALESCE(active_operation_id = ?2, FALSE) FROM sessions
                             WHERE session_id = ?1",
                            params![stored_session, stored_operation],
                            |row| row.get::<_, bool>(0),
                        )
                        .optional()
                        .map_err(sqlite_error)?
                        .unwrap_or(false);
                    transaction.commit().map_err(sqlite_error)?;
                    if should_signal {
                        after_commit()?;
                    }
                    return Ok(());
                }
                return Err(HarnessError::CancellationConflict {
                    cancellation_id,
                    operation_id: parse_operation_id(&stored_operation)?,
                });
            }

            let active_state = transaction
                .query_row(
                    "SELECT o.state_json, s.active_operation_id
                     FROM operations AS o
                     JOIN sessions AS s ON s.session_id = o.session_id
                     WHERE o.session_id = ?1 AND o.operation_id = ?2",
                    params![session_key, operation_key],
                    |row| Ok((row.get::<_, String>(0)?, row.get::<_, Option<String>>(1)?)),
                )
                .optional()
                .map_err(sqlite_error)?;
            let Some((state_json, active_operation)) = active_state else {
                return Err(HarnessError::OperationNotCancellable(operation_id));
            };
            if active_operation.as_deref() != Some(operation_key.as_str())
                || !matches!(
                    parse_state(&state_json)?.state(),
                    StoredOperationState::NeedModel { .. }
                        | StoredOperationState::ModelPending { .. }
                        | StoredOperationState::NeedTool { .. }
                        | StoredOperationState::ToolPending { .. }
                )
            {
                return Err(HarnessError::OperationNotCancellable(operation_id));
            }
            transaction
                .execute(
                    "INSERT INTO cancellation_requests (
                        cancellation_id, session_id, operation_id
                     ) VALUES (?1, ?2, ?3)",
                    params![cancellation_id.to_string(), session_key, operation_key,],
                )
                .map_err(sqlite_error)?;
            transaction.commit().map_err(sqlite_error)?;
            after_commit()?;
            Ok(())
        })
        .await
    }
}
