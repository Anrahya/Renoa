use std::sync::Arc;

use rusqlite::{OptionalExtension, Transaction, TransactionBehavior, params};

use crate::{
    Admission, HarnessError, OperationId, OperationRequest, OperationSnapshot, OutputId,
    OutputRecord, SessionId, SessionRunLease, SessionSnapshot,
    database::DatabaseLease,
    schema::{initialize, json_error, sqlite_error},
    state::StoredState,
    store_support::load_messages,
};

pub(crate) struct Store {
    database: Arc<DatabaseLease>,
}

impl Store {
    pub(crate) fn open(database: Arc<DatabaseLease>) -> Result<Self, HarnessError> {
        let mut connection = database.connection()?;
        initialize(&mut connection)?;
        Ok(Self { database })
    }

    #[cfg(test)]
    pub(crate) fn inspect_model_attempts(
        &self,
        session_id: SessionId,
    ) -> Result<Vec<crate::ModelAttemptDiagnostic>, HarnessError> {
        let connection = self.database.connection()?;
        let mut statement = connection
            .prepare(
                "SELECT a.status, a.usage_json, a.request_json IS NOT NULL, a.error
                 FROM model_attempts AS a
                 JOIN operations AS o ON o.operation_id = a.operation_id
                 WHERE o.session_id = ?1
                 ORDER BY o.position, a.attempt_number",
            )
            .map_err(sqlite_error)?;
        let rows = statement
            .query_map([session_id.to_string()], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, Option<String>>(1)?,
                    row.get::<_, bool>(2)?,
                    row.get::<_, Option<String>>(3)?,
                ))
            })
            .map_err(sqlite_error)?;
        let mut attempts = Vec::new();
        for row in rows {
            let (status, usage_json, has_request, error) = row.map_err(sqlite_error)?;
            let usage = usage_json
                .map(|value| serde_json::from_str(&value).map_err(json_error))
                .transpose()?
                .flatten();
            attempts.push(crate::ModelAttemptDiagnostic {
                status,
                usage,
                has_request,
                error,
            });
        }
        Ok(attempts)
    }

    pub(crate) fn database(&self) -> Arc<DatabaseLease> {
        Arc::clone(&self.database)
    }

    pub(crate) async fn create_session(&self, session_id: SessionId) -> Result<(), HarnessError> {
        let database = self.database();
        blocking(move || {
            let connection = database.connection()?;
            connection
                .execute(
                    "INSERT INTO sessions (
                        session_id, next_operation_position, active_operation_id,
                        next_entry_sequence, next_output_sequence
                     ) VALUES (?1, 0, NULL, 0, 0)
                     ON CONFLICT(session_id) DO NOTHING",
                    [session_id.to_string()],
                )
                .map_err(sqlite_error)?;
            Ok(())
        })
        .await
    }

    pub(crate) async fn admit(
        &self,
        session_id: SessionId,
        request: OperationRequest,
    ) -> Result<Admission, HarnessError> {
        let database = self.database();
        blocking(move || {
            let mut connection = database.connection()?;
            let transaction = connection
                .transaction_with_behavior(TransactionBehavior::Immediate)
                .map_err(sqlite_error)?;
            let request_json = serde_json::to_string(&request).map_err(json_error)?;
            let request_id = request.request_id();
            let existing = transaction
                .query_row(
                    "SELECT operation_id, position, request_json
                     FROM operations
                     WHERE session_id = ?1 AND request_id = ?2",
                    params![session_id.to_string(), request_id.to_string()],
                    |row| {
                        Ok((
                            row.get::<_, String>(0)?,
                            row.get::<_, i64>(1)?,
                            row.get::<_, String>(2)?,
                        ))
                    },
                )
                .optional()
                .map_err(sqlite_error)?;
            if let Some((operation_id, position, stored_request)) = existing {
                let operation_id = parse_operation_id(&operation_id)?;
                if stored_request != request_json {
                    return Err(HarnessError::RequestConflict {
                        request_id,
                        operation_id,
                    });
                }
                return Ok(Admission {
                    operation_id,
                    position: from_sql_integer(position, "operation position")?,
                });
            }

            let position = transaction
                .query_row(
                    "SELECT next_operation_position FROM sessions WHERE session_id = ?1",
                    [session_id.to_string()],
                    |row| row.get::<_, i64>(0),
                )
                .optional()
                .map_err(sqlite_error)?
                .ok_or(HarnessError::SessionNotFound(session_id))?;
            let operation_id = OperationId::new();
            let state_json = serde_json::to_string(&StoredState::queued()).map_err(json_error)?;
            transaction
                .execute(
                    "INSERT INTO operations (
                        operation_id, session_id, request_id, position, request_json, state_json
                     ) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                    params![
                        operation_id.to_string(),
                        session_id.to_string(),
                        request_id.to_string(),
                        position,
                        request_json,
                        state_json,
                    ],
                )
                .map_err(sqlite_error)?;
            let changed = transaction
                .execute(
                    "UPDATE sessions
                     SET next_operation_position = next_operation_position + 1
                     WHERE session_id = ?1",
                    [session_id.to_string()],
                )
                .map_err(sqlite_error)?;
            if changed != 1 {
                return Err(HarnessError::Corrupt(
                    "session admission cursor update failed".to_owned(),
                ));
            }
            transaction.commit().map_err(sqlite_error)?;
            Ok(Admission {
                operation_id,
                position: from_sql_integer(position, "operation position")?,
            })
        })
        .await
    }

    pub(crate) async fn inspect(
        &self,
        session_id: SessionId,
    ) -> Result<SessionSnapshot, HarnessError> {
        let database = self.database();
        blocking(move || {
            let mut connection = database.connection()?;
            let transaction = connection
                .transaction_with_behavior(TransactionBehavior::Deferred)
                .map_err(sqlite_error)?;
            require_session(&transaction, session_id)?;
            let snapshot = SessionSnapshot {
                messages: load_messages(&transaction, session_id)?,
                operations: load_operations(&transaction, session_id)?,
                outputs: load_outputs(&transaction, session_id)?,
            };
            transaction.commit().map_err(sqlite_error)?;
            Ok(snapshot)
        })
        .await
    }
}

fn require_session(
    transaction: &Transaction<'_>,
    session_id: SessionId,
) -> Result<(), HarnessError> {
    let exists = transaction
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM sessions WHERE session_id = ?1)",
            [session_id.to_string()],
            |row| row.get::<_, bool>(0),
        )
        .map_err(sqlite_error)?;
    if exists {
        Ok(())
    } else {
        Err(HarnessError::SessionNotFound(session_id))
    }
}

fn load_operations(
    transaction: &Transaction<'_>,
    session_id: SessionId,
) -> Result<Vec<OperationSnapshot>, HarnessError> {
    let mut statement = transaction
        .prepare(
            "SELECT operation_id, position, state_json
             FROM operations WHERE session_id = ?1 ORDER BY position",
        )
        .map_err(sqlite_error)?;
    let rows = statement
        .query_map([session_id.to_string()], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, i64>(1)?,
                row.get::<_, String>(2)?,
            ))
        })
        .map_err(sqlite_error)?;
    let mut operations = Vec::new();
    for row in rows {
        let (operation_id, position, state_json) = row.map_err(sqlite_error)?;
        let state: StoredState = serde_json::from_str(&state_json).map_err(json_error)?;
        if state.format_version() != 1 {
            return Err(HarnessError::Corrupt(format!(
                "unsupported operation state version {}",
                state.format_version()
            )));
        }
        operations.push(OperationSnapshot {
            operation_id: parse_operation_id(&operation_id)?,
            position: from_sql_integer(position, "operation position")?,
            status: state.status(),
        });
    }
    Ok(operations)
}

fn load_outputs(
    transaction: &Transaction<'_>,
    session_id: SessionId,
) -> Result<Vec<OutputRecord>, HarnessError> {
    let mut statement = transaction
        .prepare(
            "SELECT output_id, sequence, operation_id, outcome_json
             FROM outputs WHERE session_id = ?1 ORDER BY sequence",
        )
        .map_err(sqlite_error)?;
    let rows = statement
        .query_map([session_id.to_string()], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, i64>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
            ))
        })
        .map_err(sqlite_error)?;
    let mut outputs = Vec::new();
    for row in rows {
        let (output_id, sequence, operation_id, outcome_json) = row.map_err(sqlite_error)?;
        outputs.push(OutputRecord {
            output_id: parse_output_id(&output_id)?,
            sequence: from_sql_integer(sequence, "output sequence")?,
            operation_id: parse_operation_id(&operation_id)?,
            outcome: serde_json::from_str(&outcome_json).map_err(json_error)?,
        });
    }
    Ok(outputs)
}

pub(crate) async fn blocking<T, F>(operation: F) -> Result<T, HarnessError>
where
    T: Send + 'static,
    F: FnOnce() -> Result<T, HarnessError> + Send + 'static,
{
    tokio::task::spawn_blocking(operation)
        .await
        .map_err(|error| HarnessError::Store(format!("SQLite worker failed: {error}")))?
}

pub(crate) async fn blocking_transition<T, F>(
    lease: Arc<SessionRunLease>,
    operation: F,
) -> Result<T, HarnessError>
where
    T: Send + 'static,
    F: FnOnce() -> Result<T, HarnessError> + Send + 'static,
{
    blocking(move || {
        let result = operation();
        drop(lease);
        result
    })
    .await
}

pub(crate) fn parse_operation_id(value: &str) -> Result<OperationId, HarnessError> {
    value
        .parse()
        .map(OperationId::from_uuid)
        .map_err(|error| HarnessError::Corrupt(format!("invalid operation id: {error}")))
}

fn parse_output_id(value: &str) -> Result<OutputId, HarnessError> {
    value
        .parse()
        .map(OutputId::from_uuid)
        .map_err(|error| HarnessError::Corrupt(format!("invalid output id: {error}")))
}

pub(crate) fn from_sql_integer(value: i64, field: &str) -> Result<u64, HarnessError> {
    u64::try_from(value).map_err(|_| HarnessError::Corrupt(format!("negative {field}")))
}
