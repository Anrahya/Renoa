use rusqlite::{OptionalExtension, TransactionBehavior, params};
use serde_json::Value;
use tokio_util::sync::CancellationToken;

use crate::{
    Checkpoint, EffectId, EffectInvocation, EffectOutcome, EffectRecovery, EffectSnapshot,
    EffectStatus, Kernel, KernelError, OperationId, RuntimeManifest, SettledEffect,
    admission::from_sql_integer,
    cancellation::cancellation_requested,
    operation_phase::OperationPhase,
    schema::{json_error, sqlite_error},
};

pub(crate) struct PendingEffect {
    pub(crate) effect_id: EffectId,
    pub(crate) binding: String,
    pub(crate) binding_revision: String,
    pub(crate) request: Value,
    pub(crate) transition_version: i64,
}

pub(crate) struct NewEffectIntent {
    pub(crate) checkpoint: Checkpoint,
    pub(crate) binding: String,
    pub(crate) binding_revision: String,
    pub(crate) request: Value,
    pub(crate) recovery: EffectRecovery,
}

impl PendingEffect {
    pub(crate) fn into_invocation(
        self,
        runtime_manifest: RuntimeManifest,
        cancellation: CancellationToken,
    ) -> EffectInvocation {
        EffectInvocation {
            effect_id: self.effect_id,
            binding: self.binding,
            binding_revision: self.binding_revision,
            runtime_manifest,
            request: self.request,
            cancellation,
        }
    }
}

pub(crate) enum EffectStart {
    Invoke(PendingEffect),
    Blocked,
    CancellationPending,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum EffectIntentCommit {
    Committed,
    CancellationPending,
}

impl Kernel {
    pub(crate) fn commit_effect_intent(
        &self,
        operation_id: OperationId,
        expected_transition: i64,
        intent: &NewEffectIntent,
    ) -> Result<EffectIntentCommit, KernelError> {
        let mut connection = self.database.connection()?;
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(sqlite_error)?;
        if cancellation_requested(&transaction, operation_id)? {
            transaction.commit().map_err(sqlite_error)?;
            return Ok(EffectIntentCommit::CancellationPending);
        }
        let position = transaction
            .query_row(
                "SELECT next_effect_position FROM operations
                 WHERE operation_id = ?1 AND phase = 'need_decision'
                     AND transition_version = ?2",
                params![operation_id.to_string(), expected_transition],
                |row| row.get::<_, i64>(0),
            )
            .optional()
            .map_err(sqlite_error)?
            .ok_or_else(|| {
                KernelError::Corrupt("effect intent compare-and-set failed".to_owned())
            })?;
        let effect_id = EffectId::new();
        transaction
            .execute(
                "INSERT INTO effects (
                    effect_id, operation_id, position, binding, binding_revision,
                    recovery, request_json, status, dispatch_count, outcome_json
                 ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, 'intent_committed', 0, NULL)",
                params![
                    effect_id.to_string(),
                    operation_id.to_string(),
                    position,
                    &intent.binding,
                    &intent.binding_revision,
                    intent.recovery.as_str(),
                    serde_json::to_string(&intent.request).map_err(json_error)?,
                ],
            )
            .map_err(sqlite_error)?;
        let changed = transaction
            .execute(
                "UPDATE operations
                 SET phase = 'effect_intent', checkpoint_json = ?3,
                     current_effect_id = ?4, input_effect_id = NULL,
                     next_effect_position = next_effect_position + 1,
                     transition_version = transition_version + 1
                 WHERE operation_id = ?1 AND phase = 'need_decision'
                     AND transition_version = ?2",
                params![
                    operation_id.to_string(),
                    expected_transition,
                    serde_json::to_string(&intent.checkpoint).map_err(json_error)?,
                    effect_id.to_string(),
                ],
            )
            .map_err(sqlite_error)?;
        if changed != 1 {
            return Err(KernelError::Corrupt(
                "effect intent state update failed".to_owned(),
            ));
        }
        transaction.commit().map_err(sqlite_error)?;
        Ok(EffectIntentCommit::Committed)
    }

    pub(crate) fn prepare_effect(
        &self,
        operation_id: OperationId,
        expected_transition: i64,
    ) -> Result<EffectStart, KernelError> {
        let next_transition = expected_transition
            .checked_add(1)
            .ok_or_else(|| KernelError::Corrupt("transition version overflowed".to_owned()))?;
        let mut connection = self.database.connection()?;
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(sqlite_error)?;
        if cancellation_requested(&transaction, operation_id)? {
            transaction.commit().map_err(sqlite_error)?;
            return Ok(EffectStart::CancellationPending);
        }
        let (phase, current_effect_id) = transaction
            .query_row(
                "SELECT phase, current_effect_id FROM operations
                 WHERE operation_id = ?1 AND transition_version = ?2",
                params![operation_id.to_string(), expected_transition],
                |row| Ok((row.get::<_, String>(0)?, row.get::<_, Option<String>>(1)?)),
            )
            .optional()
            .map_err(sqlite_error)?
            .ok_or_else(|| {
                KernelError::Corrupt("effect state changed before dispatch".to_owned())
            })?;
        let phase = OperationPhase::from_database(&phase)?;
        let current_effect_id = current_effect_id
            .ok_or_else(|| KernelError::Corrupt("effect phase has no current effect".to_owned()))?;
        let effect_id = parse_effect_id(&current_effect_id)?;
        let (binding, revision, recovery, request, status) = transaction
            .query_row(
                "SELECT binding, binding_revision, recovery, request_json, status
                 FROM effects WHERE effect_id = ?1 AND operation_id = ?2",
                params![effect_id.to_string(), operation_id.to_string()],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, String>(2)?,
                        row.get::<_, String>(3)?,
                        row.get::<_, String>(4)?,
                    ))
                },
            )
            .optional()
            .map_err(sqlite_error)?
            .ok_or_else(|| KernelError::Corrupt("current effect is missing".to_owned()))?;
        let recovery = parse_recovery(&recovery)?;
        let expected_status = phase.active_effect_status()?;
        if status != expected_status {
            return Err(KernelError::Corrupt(
                "effect status does not match operation phase".to_owned(),
            ));
        }
        let request = serde_json::from_str(&request).map_err(json_error)?;
        if phase == OperationPhase::EffectDispatched && recovery == EffectRecovery::NeverReplay {
            mark_outcome_unknown(&transaction, operation_id, effect_id, expected_transition)?;
            transaction.commit().map_err(sqlite_error)?;
            return Ok(EffectStart::Blocked);
        }
        let changed = transaction
            .execute(
                "UPDATE effects
                 SET status = 'dispatch_started', dispatch_count = dispatch_count + 1
                 WHERE effect_id = ?1 AND status = ?2",
                params![effect_id.to_string(), expected_status],
            )
            .map_err(sqlite_error)?;
        if changed != 1 {
            return Err(KernelError::Corrupt(
                "effect dispatch compare-and-set failed".to_owned(),
            ));
        }
        let changed = transaction
            .execute(
                "UPDATE operations
                 SET phase = 'effect_dispatched', transition_version = transition_version + 1
                 WHERE operation_id = ?1 AND transition_version = ?2 AND phase = ?3",
                params![
                    operation_id.to_string(),
                    expected_transition,
                    phase.as_str()
                ],
            )
            .map_err(sqlite_error)?;
        if changed != 1 {
            return Err(KernelError::Corrupt(
                "operation dispatch compare-and-set failed".to_owned(),
            ));
        }
        transaction.commit().map_err(sqlite_error)?;
        Ok(EffectStart::Invoke(PendingEffect {
            effect_id,
            binding,
            binding_revision: revision,
            request,
            transition_version: next_transition,
        }))
    }

    pub(crate) fn settle_effect(
        &self,
        operation_id: OperationId,
        effect_id: EffectId,
        expected_transition: i64,
        outcome: &EffectOutcome,
    ) -> Result<(), KernelError> {
        let mut connection = self.database.connection()?;
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(sqlite_error)?;
        let changed = transaction
            .execute(
                "UPDATE effects SET status = 'settled', outcome_json = ?2
                 WHERE effect_id = ?1 AND operation_id = ?3
                     AND status = 'dispatch_started'",
                params![
                    effect_id.to_string(),
                    serde_json::to_string(outcome).map_err(json_error)?,
                    operation_id.to_string(),
                ],
            )
            .map_err(sqlite_error)?;
        if changed != 1 {
            return Err(KernelError::Corrupt(
                "effect settlement compare-and-set failed".to_owned(),
            ));
        }
        let changed = transaction
            .execute(
                "UPDATE operations
                 SET phase = 'need_decision', current_effect_id = NULL,
                     input_effect_id = ?3, transition_version = transition_version + 1
                 WHERE operation_id = ?1 AND phase = 'effect_dispatched'
                     AND transition_version = ?2 AND current_effect_id = ?3",
                params![
                    operation_id.to_string(),
                    expected_transition,
                    effect_id.to_string(),
                ],
            )
            .map_err(sqlite_error)?;
        if changed != 1 {
            return Err(KernelError::Corrupt(
                "operation effect settlement compare-and-set failed".to_owned(),
            ));
        }
        transaction.commit().map_err(sqlite_error)
    }

    pub(crate) fn record_outcome_unknown(
        &self,
        operation_id: OperationId,
        effect_id: EffectId,
        expected_transition: i64,
    ) -> Result<(), KernelError> {
        let mut connection = self.database.connection()?;
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(sqlite_error)?;
        mark_outcome_unknown(&transaction, operation_id, effect_id, expected_transition)?;
        transaction.commit().map_err(sqlite_error)
    }

    pub(crate) fn load_settled_effect(
        &self,
        effect_id: EffectId,
        operation_id: OperationId,
    ) -> Result<SettledEffect, KernelError> {
        let connection = self.database.connection()?;
        let (binding, binding_revision, request, outcome) = connection
            .query_row(
                "SELECT binding, binding_revision, request_json, outcome_json FROM effects
                 WHERE effect_id = ?1 AND operation_id = ?2 AND status = 'settled'",
                params![effect_id.to_string(), operation_id.to_string()],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, String>(2)?,
                        row.get::<_, String>(3)?,
                    ))
                },
            )
            .optional()
            .map_err(sqlite_error)?
            .ok_or_else(|| KernelError::Corrupt("settled input effect is missing".to_owned()))?;
        Ok(SettledEffect {
            effect_id,
            binding,
            binding_revision,
            request: serde_json::from_str(&request).map_err(json_error)?,
            outcome: serde_json::from_str(&outcome).map_err(json_error)?,
        })
    }
}

pub(crate) fn mark_outcome_unknown(
    transaction: &rusqlite::Transaction<'_>,
    operation_id: OperationId,
    effect_id: EffectId,
    expected_transition: i64,
) -> Result<(), KernelError> {
    let changed = transaction
        .execute(
            "UPDATE effects SET status = 'outcome_unknown'
             WHERE effect_id = ?1 AND status = 'dispatch_started'",
            [effect_id.to_string()],
        )
        .map_err(sqlite_error)?;
    if changed != 1 {
        return Err(KernelError::Corrupt(
            "unknown effect compare-and-set failed".to_owned(),
        ));
    }
    let changed = transaction
        .execute(
            "UPDATE operations
             SET phase = 'outcome_unknown', transition_version = transition_version + 1
             WHERE operation_id = ?1 AND phase = 'effect_dispatched'
                 AND transition_version = ?2 AND current_effect_id = ?3",
            params![
                operation_id.to_string(),
                expected_transition,
                effect_id.to_string(),
            ],
        )
        .map_err(sqlite_error)?;
    if changed == 1 {
        Ok(())
    } else {
        Err(KernelError::Corrupt(
            "unknown operation compare-and-set failed".to_owned(),
        ))
    }
}

pub(crate) fn load_effect_snapshots(
    connection: &rusqlite::Connection,
    operation_id: OperationId,
) -> Result<Vec<EffectSnapshot>, KernelError> {
    let mut statement = connection
        .prepare(
            "SELECT effect_id, position, binding, binding_revision, recovery,
                    request_json, status, dispatch_count, outcome_json
             FROM effects WHERE operation_id = ?1 ORDER BY position",
        )
        .map_err(sqlite_error)?;
    let rows = statement
        .query_map([operation_id.to_string()], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, i64>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, String>(4)?,
                row.get::<_, String>(5)?,
                row.get::<_, String>(6)?,
                row.get::<_, i64>(7)?,
                row.get::<_, Option<String>>(8)?,
            ))
        })
        .map_err(sqlite_error)?;
    let mut effects = Vec::new();
    for row in rows {
        let (id, position, binding, revision, recovery, request, status, dispatches, outcome) =
            row.map_err(sqlite_error)?;
        effects.push(EffectSnapshot {
            effect_id: parse_effect_id(&id)?,
            position: from_sql_integer(position, "effect position")?,
            binding,
            binding_revision: revision,
            recovery: parse_recovery(&recovery)?,
            request: serde_json::from_str(&request).map_err(json_error)?,
            status: parse_status(&status)?,
            dispatch_count: from_sql_integer(dispatches, "effect dispatch count")?,
            outcome: outcome
                .map(|value| serde_json::from_str(&value).map_err(json_error))
                .transpose()?,
        });
    }
    Ok(effects)
}

pub(crate) fn parse_effect_id(value: &str) -> Result<EffectId, KernelError> {
    uuid::Uuid::parse_str(value)
        .map(EffectId::from_uuid)
        .map_err(|error| KernelError::Corrupt(format!("invalid effect id: {error}")))
}

fn parse_recovery(value: &str) -> Result<EffectRecovery, KernelError> {
    match value {
        "safe_to_replay" => Ok(EffectRecovery::SafeToReplay),
        "never_replay" => Ok(EffectRecovery::NeverReplay),
        _ => Err(KernelError::Corrupt(format!(
            "unknown effect recovery `{value}`"
        ))),
    }
}

fn parse_status(value: &str) -> Result<EffectStatus, KernelError> {
    match value {
        "intent_committed" => Ok(EffectStatus::IntentCommitted),
        "dispatch_started" => Ok(EffectStatus::DispatchStarted),
        "settled" => Ok(EffectStatus::Settled),
        "outcome_unknown" => Ok(EffectStatus::OutcomeUnknown),
        _ => Err(KernelError::Corrupt(format!(
            "unknown effect status `{value}`"
        ))),
    }
}
