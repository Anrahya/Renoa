use rusqlite::OptionalExtension as _;

use crate::{HarnessError, OperationId, OperationOutcome, SessionId, store::Store};

impl Store {
    pub(crate) async fn settled_outcome(
        &self,
        session_id: SessionId,
        operation_id: OperationId,
    ) -> Result<Option<OperationOutcome>, HarnessError> {
        let database = self.database();
        crate::store::blocking(move || {
            let connection = database.connection()?;
            let outcome = connection
                .query_row(
                    "SELECT outcome_json FROM outputs
                     WHERE session_id = ?1 AND operation_id = ?2",
                    [session_id.to_string(), operation_id.to_string()],
                    |row| row.get::<_, String>(0),
                )
                .optional()
                .map_err(crate::schema::sqlite_error)?;
            if let Some(outcome) = outcome {
                return serde_json::from_str(&outcome)
                    .map(Some)
                    .map_err(crate::schema::json_error);
            }
            let session_exists = connection
                .query_row(
                    "SELECT EXISTS(SELECT 1 FROM sessions WHERE session_id = ?1)",
                    [session_id.to_string()],
                    |row| row.get::<_, bool>(0),
                )
                .map_err(crate::schema::sqlite_error)?;
            if session_exists {
                Ok(None)
            } else {
                Err(HarnessError::SessionNotFound(session_id))
            }
        })
        .await
    }
}
