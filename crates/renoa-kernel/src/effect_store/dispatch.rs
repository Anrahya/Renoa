use rusqlite::{OptionalExtension, TransactionBehavior, params};

use crate::{
    EffectBatchId, EffectId, EffectRecovery, EffectStatus, Kernel, KernelError, OperationId,
    admission::from_sql_integer,
    cancellation::cancellation_requested,
    operation_phase::OperationPhase,
    schema::{json_error, sqlite_error},
};

use super::{
    EffectBatchStart, EffectIntentCommit, NewEffectBatchIntent, PendingEffect, PendingEffectBatch,
    parse_effect_batch_id, parse_effect_id, parse_recovery, parse_status, status_name,
    transition_finished_batch, update_effect_status,
};

impl Kernel {
    pub(crate) fn commit_effect_batch_intent(
        &self,
        operation_id: OperationId,
        expected_transition: i64,
        intent: &NewEffectBatchIntent,
    ) -> Result<EffectIntentCommit, KernelError> {
        if intent.effects.is_empty() {
            return Err(KernelError::InvalidDecision(
                "an effect batch must contain at least one effect".to_owned(),
            ));
        }
        let checkpoint_json = serde_json::to_string(&intent.checkpoint).map_err(json_error)?;
        let requests = intent
            .effects
            .iter()
            .map(|effect| serde_json::to_string(&effect.request).map_err(json_error))
            .collect::<Result<Vec<_>, _>>()?;
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
                "SELECT next_effect_batch_position FROM operations
                 WHERE operation_id = ?1 AND phase = 'need_decision'
                     AND transition_version = ?2",
                params![operation_id.to_string(), expected_transition],
                |row| row.get::<_, i64>(0),
            )
            .optional()
            .map_err(sqlite_error)?
            .ok_or_else(|| {
                KernelError::Corrupt("effect batch intent compare-and-set failed".to_owned())
            })?;
        let batch_id = EffectBatchId::new();
        transaction
            .execute(
                "INSERT INTO effect_batches (batch_id, operation_id, position)
                 VALUES (?1, ?2, ?3)",
                params![batch_id.to_string(), operation_id.to_string(), position],
            )
            .map_err(sqlite_error)?;
        for (position, (effect, request_json)) in intent.effects.iter().zip(requests).enumerate() {
            let position = i64::try_from(position).map_err(|error| {
                KernelError::Corrupt(format!("effect position exceeds i64: {error}"))
            })?;
            transaction
                .execute(
                    "INSERT INTO effects (
                        effect_id, batch_id, position, binding, binding_revision,
                        recovery, request_json, status, dispatch_count, outcome_json
                     ) VALUES (
                        ?1, ?2, ?3, ?4, ?5, ?6, ?7,
                        'intent_committed', 0, NULL
                     )",
                    params![
                        EffectId::new().to_string(),
                        batch_id.to_string(),
                        position,
                        &effect.binding,
                        &effect.binding_revision,
                        effect.recovery.as_str(),
                        request_json,
                    ],
                )
                .map_err(sqlite_error)?;
        }
        let changed = transaction
            .execute(
                "UPDATE operations
                 SET phase = 'effect_intent', checkpoint_json = ?3,
                     current_effect_batch_id = ?4, input_effect_batch_id = NULL,
                     next_effect_batch_position = next_effect_batch_position + 1,
                     transition_version = transition_version + 1
                 WHERE operation_id = ?1 AND phase = 'need_decision'
                     AND transition_version = ?2",
                params![
                    operation_id.to_string(),
                    expected_transition,
                    checkpoint_json,
                    batch_id.to_string(),
                ],
            )
            .map_err(sqlite_error)?;
        if changed != 1 {
            return Err(KernelError::Corrupt(
                "effect batch intent state update failed".to_owned(),
            ));
        }
        transaction.commit().map_err(sqlite_error)?;
        Ok(EffectIntentCommit::Committed)
    }

    pub(crate) fn prepare_effect_batch(
        &self,
        operation_id: OperationId,
        expected_transition: i64,
    ) -> Result<EffectBatchStart, KernelError> {
        let next_transition = expected_transition
            .checked_add(1)
            .ok_or_else(|| KernelError::Corrupt("transition version overflowed".to_owned()))?;
        let mut connection = self.database.connection()?;
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(sqlite_error)?;
        if cancellation_requested(&transaction, operation_id)? {
            transaction.commit().map_err(sqlite_error)?;
            return Ok(EffectBatchStart::CancellationPending);
        }
        let (phase, batch_id) = load_active_batch(
            &transaction,
            operation_id,
            expected_transition,
            "effect state changed before batch dispatch",
        )?;
        if !matches!(
            phase,
            OperationPhase::EffectIntent | OperationPhase::EffectDispatched
        ) {
            return Err(KernelError::Corrupt(format!(
                "cannot prepare effect batch from phase `{}`",
                phase.as_str()
            )));
        }
        let mut stored = load_prepared_effects(&transaction, operation_id, batch_id)?;
        let mut pending = Vec::new();
        for effect in &mut stored {
            if let Some(effect) =
                prepare_child(&transaction, phase, batch_id, next_transition, effect)?
            {
                pending.push(effect);
            }
        }
        if pending.is_empty() {
            let start = finish_batch_without_invocations(
                &transaction,
                operation_id,
                batch_id,
                expected_transition,
                &stored,
            )?;
            transaction.commit().map_err(sqlite_error)?;
            return Ok(start);
        }
        let changed = transaction
            .execute(
                "UPDATE operations
                 SET phase = 'effect_dispatched', transition_version = transition_version + 1
                 WHERE operation_id = ?1 AND transition_version = ?2
                   AND phase = ?3 AND current_effect_batch_id = ?4",
                params![
                    operation_id.to_string(),
                    expected_transition,
                    phase.as_str(),
                    batch_id.to_string(),
                ],
            )
            .map_err(sqlite_error)?;
        if changed != 1 {
            return Err(KernelError::Corrupt(
                "operation batch dispatch compare-and-set failed".to_owned(),
            ));
        }
        transaction.commit().map_err(sqlite_error)?;
        Ok(EffectBatchStart::Invoke(PendingEffectBatch {
            batch_id,
            effects: pending,
        }))
    }
}

fn prepare_child(
    transaction: &rusqlite::Transaction<'_>,
    phase: OperationPhase,
    batch_id: EffectBatchId,
    next_transition: i64,
    effect: &mut PreparedEffect,
) -> Result<Option<PendingEffect>, KernelError> {
    let should_dispatch = match (phase, effect.status, effect.recovery) {
        (OperationPhase::EffectIntent, EffectStatus::IntentCommitted, _)
        | (
            OperationPhase::EffectDispatched,
            EffectStatus::DispatchStarted,
            EffectRecovery::SafeToReplay,
        ) => true,
        (OperationPhase::EffectIntent, _, _) => {
            return Err(KernelError::Corrupt(
                "new effect batch contains a non-intent child".to_owned(),
            ));
        }
        (
            OperationPhase::EffectDispatched,
            EffectStatus::DispatchStarted,
            EffectRecovery::NeverReplay,
        ) => {
            update_effect_status(
                transaction,
                effect.effect_id,
                EffectStatus::DispatchStarted,
                EffectStatus::OutcomeUnknown,
            )?;
            effect.status = EffectStatus::OutcomeUnknown;
            false
        }
        (
            OperationPhase::EffectDispatched,
            EffectStatus::Settled | EffectStatus::OutcomeUnknown,
            _,
        ) => false,
        (OperationPhase::EffectDispatched, EffectStatus::IntentCommitted, _) => {
            return Err(KernelError::Corrupt(
                "dispatched effect batch contains an unmarked child".to_owned(),
            ));
        }
        _ => unreachable!("phase is checked before child preparation"),
    };
    if !should_dispatch {
        return Ok(None);
    }
    let dispatch_count = effect
        .dispatch_count
        .checked_add(1)
        .ok_or_else(|| KernelError::Corrupt("effect dispatch count overflowed".to_owned()))?;
    let current_count = i64::try_from(effect.dispatch_count).map_err(|error| {
        KernelError::Corrupt(format!("effect dispatch count exceeds i64: {error}"))
    })?;
    let next_count = i64::try_from(dispatch_count).map_err(|error| {
        KernelError::Corrupt(format!("effect dispatch count exceeds i64: {error}"))
    })?;
    let changed = transaction
        .execute(
            "UPDATE effects
             SET status = 'dispatch_started', dispatch_count = ?3
             WHERE effect_id = ?1 AND status = ?2 AND dispatch_count = ?4",
            params![
                effect.effect_id.to_string(),
                status_name(effect.status),
                next_count,
                current_count,
            ],
        )
        .map_err(sqlite_error)?;
    if changed != 1 {
        return Err(KernelError::Corrupt(
            "effect dispatch compare-and-set failed".to_owned(),
        ));
    }
    effect.status = EffectStatus::DispatchStarted;
    effect.dispatch_count = dispatch_count;
    Ok(Some(PendingEffect {
        batch_id,
        effect_id: effect.effect_id,
        binding: effect.binding.clone(),
        binding_revision: effect.binding_revision.clone(),
        request: effect.request.clone(),
        recovery: effect.recovery,
        dispatch_count,
        transition_version: next_transition,
    }))
}

fn finish_batch_without_invocations(
    transaction: &rusqlite::Transaction<'_>,
    operation_id: OperationId,
    batch_id: EffectBatchId,
    expected_transition: i64,
    effects: &[PreparedEffect],
) -> Result<EffectBatchStart, KernelError> {
    if effects
        .iter()
        .any(|effect| effect.status == EffectStatus::DispatchStarted)
    {
        return Err(KernelError::Corrupt(
            "effect batch has pending children but no dispatch".to_owned(),
        ));
    }
    let blocked = effects
        .iter()
        .any(|effect| effect.status == EffectStatus::OutcomeUnknown);
    transition_finished_batch(
        transaction,
        operation_id,
        batch_id,
        expected_transition,
        blocked,
    )?;
    Ok(if blocked {
        EffectBatchStart::Blocked
    } else {
        EffectBatchStart::Settled
    })
}

fn load_active_batch(
    transaction: &rusqlite::Transaction<'_>,
    operation_id: OperationId,
    expected_transition: i64,
    error: &'static str,
) -> Result<(OperationPhase, EffectBatchId), KernelError> {
    let (phase, batch_id) = transaction
        .query_row(
            "SELECT phase, current_effect_batch_id FROM operations
             WHERE operation_id = ?1 AND transition_version = ?2",
            params![operation_id.to_string(), expected_transition],
            |row| Ok((row.get::<_, String>(0)?, row.get::<_, Option<String>>(1)?)),
        )
        .optional()
        .map_err(sqlite_error)?
        .ok_or_else(|| KernelError::Corrupt(error.to_owned()))?;
    let batch_id = batch_id
        .ok_or_else(|| KernelError::Corrupt("effect phase has no current batch".to_owned()))?;
    Ok((
        OperationPhase::from_database(&phase)?,
        parse_effect_batch_id(&batch_id)?,
    ))
}

struct PreparedEffect {
    effect_id: EffectId,
    binding: String,
    binding_revision: String,
    recovery: EffectRecovery,
    request: serde_json::Value,
    status: EffectStatus,
    dispatch_count: u64,
}

fn load_prepared_effects(
    transaction: &rusqlite::Transaction<'_>,
    operation_id: OperationId,
    batch_id: EffectBatchId,
) -> Result<Vec<PreparedEffect>, KernelError> {
    let mut statement = transaction
        .prepare(
            "SELECT e.effect_id, e.position, e.binding, e.binding_revision,
                    e.recovery, e.request_json, e.status, e.dispatch_count
             FROM effect_batches AS b
             JOIN effects AS e ON e.batch_id = b.batch_id
             WHERE b.operation_id = ?1 AND b.batch_id = ?2
             ORDER BY e.position",
        )
        .map_err(sqlite_error)?;
    let rows = statement
        .query_map(
            params![operation_id.to_string(), batch_id.to_string()],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, i64>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, String>(4)?,
                    row.get::<_, String>(5)?,
                    row.get::<_, String>(6)?,
                    row.get::<_, i64>(7)?,
                ))
            },
        )
        .map_err(sqlite_error)?;
    let mut effects = Vec::new();
    for (expected_position, row) in rows.enumerate() {
        let (effect_id, position, binding, revision, recovery, request, status, dispatch_count) =
            row.map_err(sqlite_error)?;
        if from_sql_integer(position, "effect position")? != expected_position as u64 {
            return Err(KernelError::Corrupt(
                "effect batch child positions are not gapless".to_owned(),
            ));
        }
        effects.push(PreparedEffect {
            effect_id: parse_effect_id(&effect_id)?,
            binding,
            binding_revision: revision,
            recovery: parse_recovery(&recovery)?,
            request: serde_json::from_str(&request).map_err(json_error)?,
            status: parse_status(&status)?,
            dispatch_count: from_sql_integer(dispatch_count, "effect dispatch count")?,
        });
    }
    if effects.is_empty() {
        return Err(KernelError::Corrupt(
            "effect batch contains no children".to_owned(),
        ));
    }
    Ok(effects)
}
