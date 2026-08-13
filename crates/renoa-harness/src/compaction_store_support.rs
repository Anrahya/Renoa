use renoa_agent::TokenUsage;
use rusqlite::{OptionalExtension, Transaction, params};
use uuid::Uuid;

use crate::{
    HarnessError, OperationId,
    checkpoint::load_active_checkpoint,
    compaction::{CompactionIntent, CompactionPlan},
    schema::{json_error, sqlite_error},
    state::{OperationProgress, StoredOperationState},
    store_support::{parse_session_id, parse_state},
};

pub(crate) fn insert_attempt(
    transaction: &Transaction<'_>,
    operation_id: OperationId,
    effect_id: Uuid,
    settlement_token: Uuid,
    attempt_number: u32,
    plan: &CompactionPlan,
) -> Result<(), HarnessError> {
    transaction
        .execute(
            "INSERT INTO compaction_attempts (
                effect_id, operation_id, attempt_number, settlement_token,
                checkpoint_id, previous_checkpoint_id, covered_through_sequence,
                status, request_json, usage_json
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, 'pending', ?8, NULL)",
            params![
                effect_id.to_string(),
                operation_id.to_string(),
                i64::from(attempt_number),
                settlement_token.to_string(),
                plan.checkpoint_id.to_string(),
                plan.previous_checkpoint_id.map(|id| id.to_string()),
                i64::try_from(plan.covered_through_sequence).map_err(|_| {
                    HarnessError::Corrupt("checkpoint sequence exceeds i64".to_owned())
                })?,
                serde_json::to_string(&plan.request).map_err(json_error)?,
            ],
        )
        .map_err(sqlite_error)?;
    Ok(())
}

pub(crate) fn complete_attempt(
    transaction: &Transaction<'_>,
    intent: &CompactionIntent,
    usage: Option<TokenUsage>,
) -> Result<(), HarnessError> {
    let changed = transaction
        .execute(
            "UPDATE compaction_attempts
             SET status = 'completed', request_json = NULL, usage_json = ?3
             WHERE effect_id = ?1 AND settlement_token = ?2 AND status = 'pending'",
            params![
                intent.effect_id.to_string(),
                intent.settlement_token.to_string(),
                serde_json::to_string(&usage).map_err(json_error)?,
            ],
        )
        .map_err(sqlite_error)?;
    require_one(changed, "compaction settlement")
}

pub(crate) fn mark_attempt_unknown(
    transaction: &Transaction<'_>,
    intent: &CompactionIntent,
) -> Result<(), HarnessError> {
    let changed = transaction
        .execute(
            "UPDATE compaction_attempts
             SET status = 'outcome_unknown', request_json = NULL
             WHERE effect_id = ?1 AND settlement_token = ?2 AND status = 'pending'",
            params![
                intent.effect_id.to_string(),
                intent.settlement_token.to_string()
            ],
        )
        .map_err(sqlite_error)?;
    require_one(changed, "uncertain compaction")
}

pub(crate) fn current_pending_state(
    transaction: &Transaction<'_>,
    intent: &CompactionIntent,
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
        StoredOperationState::CompactionPending {
            effect_id,
            settlement_token,
            checkpoint_id,
            ..
        } if *effect_id == intent.effect_id
            && *settlement_token == intent.settlement_token
            && *checkpoint_id == intent.plan.checkpoint_id =>
        {
            Ok(Some(state_json))
        }
        _ => Ok(None),
    }
}

pub(crate) fn load_pending_intent(
    transaction: &Transaction<'_>,
    operation_id: OperationId,
) -> Result<(CompactionIntent, String), HarnessError> {
    let (session_id, state_json) = load_operation(transaction, operation_id)?;
    let state = parse_state(&state_json)?;
    let StoredOperationState::CompactionPending {
        progress,
        effect_id,
        settlement_token,
        checkpoint_id,
        output_id,
    } = state.state()
    else {
        return Err(HarnessError::Corrupt(
            "compaction recovery requires CompactionPending state".to_owned(),
        ));
    };
    let row = transaction
        .query_row(
            "SELECT settlement_token, checkpoint_id, previous_checkpoint_id,
                    covered_through_sequence, request_json, status
             FROM compaction_attempts
             WHERE effect_id = ?1 AND operation_id = ?2",
            params![effect_id.to_string(), operation_id.to_string()],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, Option<String>>(2)?,
                    row.get::<_, i64>(3)?,
                    row.get::<_, Option<String>>(4)?,
                    row.get::<_, String>(5)?,
                ))
            },
        )
        .optional()
        .map_err(sqlite_error)?
        .ok_or_else(|| HarnessError::Corrupt("pending compaction attempt is missing".to_owned()))?;
    let (stored_token, stored_checkpoint, previous, covered, request, status) = row;
    if status != "pending"
        || stored_token != settlement_token.to_string()
        || stored_checkpoint != checkpoint_id.to_string()
    {
        return Err(HarnessError::Corrupt(
            "pending compaction attempt does not match operation state".to_owned(),
        ));
    }
    let request = request
        .ok_or_else(|| HarnessError::Corrupt("pending compaction request is missing".to_owned()))?;
    let plan = CompactionPlan {
        request: serde_json::from_str(&request).map_err(json_error)?,
        checkpoint_id: *checkpoint_id,
        previous_checkpoint_id: previous
            .map(|value| {
                value.parse().map_err(|error| {
                    HarnessError::Corrupt(format!("invalid previous checkpoint id: {error}"))
                })
            })
            .transpose()?,
        covered_through_sequence: u64::try_from(covered).map_err(|_| {
            HarnessError::Corrupt("checkpoint has a negative transcript sequence".to_owned())
        })?,
    };
    Ok((
        CompactionIntent {
            session_id,
            operation_id,
            effect_id: *effect_id,
            settlement_token: *settlement_token,
            output_id: *output_id,
            progress: progress.clone(),
            plan,
        },
        state_json,
    ))
}

pub(crate) fn activate_checkpoint(
    transaction: &Transaction<'_>,
    intent: &CompactionIntent,
) -> Result<(), HarnessError> {
    let expected = intent.plan.previous_checkpoint_id.map(|id| id.to_string());
    let changed = transaction
        .execute(
            "UPDATE sessions SET active_checkpoint_id = ?3
             WHERE session_id = ?1 AND active_operation_id = ?2
               AND ((active_checkpoint_id IS NULL AND ?4 IS NULL)
                    OR active_checkpoint_id = ?4)",
            params![
                intent.session_id.to_string(),
                intent.operation_id.to_string(),
                intent.plan.checkpoint_id.to_string(),
                expected,
            ],
        )
        .map_err(sqlite_error)?;
    require_one(changed, "checkpoint activation")
}

pub(crate) fn validate_plan(
    transaction: &Transaction<'_>,
    session_id: crate::SessionId,
    plan: &CompactionPlan,
) -> Result<(), HarnessError> {
    let current = load_active_checkpoint(transaction, session_id)?;
    if current.as_ref().map(|checkpoint| checkpoint.checkpoint_id) != plan.previous_checkpoint_id {
        return Err(HarnessError::Corrupt(
            "compaction plan was built from a stale checkpoint".to_owned(),
        ));
    }
    if current.as_ref().is_some_and(|checkpoint| {
        checkpoint.covered_through_sequence >= plan.covered_through_sequence
    }) {
        return Err(HarnessError::Corrupt(
            "compaction plan does not advance the checkpoint".to_owned(),
        ));
    }
    let exists = transaction
        .query_row(
            "SELECT EXISTS(
                SELECT 1 FROM conversation_entries
                WHERE session_id = ?1 AND sequence = ?2
             )",
            params![
                session_id.to_string(),
                i64::try_from(plan.covered_through_sequence).map_err(|_| {
                    HarnessError::Corrupt("checkpoint sequence exceeds i64".to_owned())
                })?,
            ],
            |row| row.get::<_, bool>(0),
        )
        .map_err(sqlite_error)?;
    if !exists {
        return Err(HarnessError::Corrupt(
            "compaction boundary is not a transcript entry".to_owned(),
        ));
    }
    Ok(())
}

pub(crate) fn load_operation(
    transaction: &Transaction<'_>,
    operation_id: OperationId,
) -> Result<(crate::SessionId, String), HarnessError> {
    let (session_id, state_json) = transaction
        .query_row(
            "SELECT session_id, state_json FROM operations WHERE operation_id = ?1",
            [operation_id.to_string()],
            |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
        )
        .optional()
        .map_err(sqlite_error)?
        .ok_or_else(|| HarnessError::Corrupt("active operation is missing".to_owned()))?;
    Ok((parse_session_id(&session_id)?, state_json))
}

pub(crate) fn next_attempt(progress: &OperationProgress) -> Result<u32, HarnessError> {
    progress
        .compaction_attempts
        .checked_add(1)
        .ok_or_else(|| HarnessError::Corrupt("compaction counter overflowed".to_owned()))
}

fn require_one(changed: usize, action: &str) -> Result<(), HarnessError> {
    if changed == 1 {
        Ok(())
    } else {
        Err(HarnessError::Corrupt(format!(
            "{action} compare-and-set failed"
        )))
    }
}
