use renoa_agent::{Message, ModelRequest};
use rusqlite::{OptionalExtension, Transaction, params};
use uuid::Uuid;

use crate::{
    HarnessError, OperationId, OperationOutcome, SessionId,
    drive::ModelIntent,
    schema::{json_error, sqlite_error},
    state::{StoredOperationState, StoredState},
};

pub(crate) fn current_pending_state(
    transaction: &Transaction<'_>,
    intent: &ModelIntent,
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
        StoredOperationState::ModelPending {
            effect_id,
            settlement_token,
            ..
        } if *effect_id == intent.effect_id && *settlement_token == intent.settlement_token => {
            Ok(Some(state_json))
        }
        _ => Ok(None),
    }
}

pub(crate) fn insert_retry_intent(
    transaction: &Transaction<'_>,
    previous: &ModelIntent,
    old_state_json: &str,
    request_json: &str,
) -> Result<ModelIntent, HarnessError> {
    let attempt_count = previous
        .attempt_count
        .checked_add(1)
        .ok_or_else(|| HarnessError::Corrupt("model attempt counter overflowed".to_owned()))?;
    let effect_id = Uuid::new_v4();
    let settlement_token = Uuid::new_v4();
    let assistant_entry_id = Uuid::new_v4();
    let output_id = Uuid::new_v4();
    transaction
        .execute(
            "INSERT INTO model_attempts (
                effect_id, operation_id, attempt_number, settlement_token, status,
                request_json, usage_json, error
             ) VALUES (?1, ?2, ?3, ?4, 'pending', ?5, NULL, NULL)",
            params![
                effect_id.to_string(),
                previous.operation_id.to_string(),
                i64::from(attempt_count),
                settlement_token.to_string(),
                request_json,
            ],
        )
        .map_err(sqlite_error)?;
    let state = StoredState::from_state(StoredOperationState::ModelPending {
        runtime_revision: previous.runtime_revision.clone(),
        max_model_attempts: previous.max_model_attempts,
        attempt_count,
        effect_id,
        settlement_token,
        assistant_entry_id,
        output_id,
    });
    update_state(
        transaction,
        previous.operation_id,
        old_state_json,
        &serde_json::to_string(&state).map_err(json_error)?,
    )?;
    Ok(ModelIntent {
        session_id: previous.session_id,
        operation_id: previous.operation_id,
        effect_id,
        settlement_token,
        assistant_entry_id,
        output_id,
        runtime_revision: previous.runtime_revision.clone(),
        max_model_attempts: previous.max_model_attempts,
        attempt_count,
        request: serde_json::from_str::<ModelRequest>(request_json).map_err(json_error)?,
    })
}

pub(crate) fn finish_failed_operation(
    transaction: &Transaction<'_>,
    intent: &ModelIntent,
    old_state_json: &str,
    message: String,
) -> Result<OperationOutcome, HarnessError> {
    let (_, output_sequence) = load_cursors(transaction, intent.session_id)?;
    let outcome = OperationOutcome::Failed { message };
    insert_output(transaction, intent, output_sequence, &outcome)?;
    let state = StoredState::from_state(StoredOperationState::Failed);
    update_state(
        transaction,
        intent.operation_id,
        old_state_json,
        &serde_json::to_string(&state).map_err(json_error)?,
    )?;
    finish_session(transaction, intent, false)?;
    Ok(outcome)
}

pub(crate) fn load_cursors(
    transaction: &Transaction<'_>,
    session_id: SessionId,
) -> Result<(i64, i64), HarnessError> {
    transaction
        .query_row(
            "SELECT next_entry_sequence, next_output_sequence
             FROM sessions WHERE session_id = ?1",
            [session_id.to_string()],
            |row| Ok((row.get::<_, i64>(0)?, row.get::<_, i64>(1)?)),
        )
        .map_err(sqlite_error)
}

pub(crate) fn load_messages(
    transaction: &Transaction<'_>,
    session_id: SessionId,
) -> Result<Vec<Message>, HarnessError> {
    let mut statement = transaction
        .prepare(
            "SELECT message_json FROM conversation_entries
             WHERE session_id = ?1 ORDER BY sequence",
        )
        .map_err(sqlite_error)?;
    let rows = statement
        .query_map([session_id.to_string()], |row| row.get::<_, String>(0))
        .map_err(sqlite_error)?;
    let mut messages = Vec::new();
    for row in rows {
        messages.push(serde_json::from_str(&row.map_err(sqlite_error)?).map_err(json_error)?);
    }
    Ok(messages)
}

pub(crate) fn insert_output(
    transaction: &Transaction<'_>,
    intent: &ModelIntent,
    sequence: i64,
    outcome: &OperationOutcome,
) -> Result<(), HarnessError> {
    transaction
        .execute(
            "INSERT INTO outputs (
                output_id, session_id, operation_id, sequence, outcome_json
             ) VALUES (?1, ?2, ?3, ?4, ?5)",
            params![
                intent.output_id.to_string(),
                intent.session_id.to_string(),
                intent.operation_id.to_string(),
                sequence,
                serde_json::to_string(outcome).map_err(json_error)?,
            ],
        )
        .map_err(sqlite_error)?;
    Ok(())
}

pub(crate) fn update_state(
    transaction: &Transaction<'_>,
    operation_id: OperationId,
    old_state_json: &str,
    new_state_json: &str,
) -> Result<(), HarnessError> {
    let changed = transaction
        .execute(
            "UPDATE operations SET state_json = ?2
             WHERE operation_id = ?1 AND state_json = ?3",
            params![operation_id.to_string(), new_state_json, old_state_json],
        )
        .map_err(sqlite_error)?;
    if changed != 1 {
        return Err(HarnessError::Corrupt(
            "operation state compare-and-set failed".to_owned(),
        ));
    }
    Ok(())
}

pub(crate) fn finish_session(
    transaction: &Transaction<'_>,
    intent: &ModelIntent,
    inserted_entry: bool,
) -> Result<(), HarnessError> {
    let changed = transaction
        .execute(
            "UPDATE sessions
             SET active_operation_id = NULL,
                 next_entry_sequence = next_entry_sequence + ?3,
                 next_output_sequence = next_output_sequence + 1
             WHERE session_id = ?1 AND active_operation_id = ?2",
            params![
                intent.session_id.to_string(),
                intent.operation_id.to_string(),
                i64::from(inserted_entry),
            ],
        )
        .map_err(sqlite_error)?;
    if changed != 1 {
        return Err(HarnessError::Corrupt(
            "active session settlement compare-and-set failed".to_owned(),
        ));
    }
    Ok(())
}

pub(crate) fn parse_state(value: &str) -> Result<StoredState, HarnessError> {
    let state: StoredState = serde_json::from_str(value).map_err(json_error)?;
    if state.format_version() != 1 {
        return Err(HarnessError::Corrupt(format!(
            "unsupported operation state version {}",
            state.format_version()
        )));
    }
    Ok(state)
}

pub(crate) fn parse_session_id(value: &str) -> Result<SessionId, HarnessError> {
    value
        .parse()
        .map(SessionId::from_uuid)
        .map_err(|error| HarnessError::Corrupt(format!("invalid session id: {error}")))
}
