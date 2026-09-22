use rusqlite::{OptionalExtension, params};

use crate::{
    EffectBatchFacts, EffectBatchId, EffectBatchSnapshot, EffectFact, EffectOutcome,
    EffectSnapshot, EffectStatus, KernelError, OperationId, RuntimeManifest, SettledEffect,
    SettledEffectBatch, UnsettledEffect,
    admission::from_sql_integer,
    schema::{json_error, sqlite_error},
};

use super::{parse_effect_batch_id, parse_effect_id, parse_recovery, parse_status};

pub(crate) fn load_effect_batch_snapshots(
    connection: &rusqlite::Connection,
    operation_id: OperationId,
) -> Result<Vec<EffectBatchSnapshot>, KernelError> {
    let mut statement = connection
        .prepare(
            "SELECT batch_id, position FROM effect_batches
             WHERE operation_id = ?1 ORDER BY position",
        )
        .map_err(sqlite_error)?;
    let rows = statement
        .query_map([operation_id.to_string()], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?))
        })
        .map_err(sqlite_error)?;
    let mut batches = Vec::new();
    for (expected_position, row) in rows.enumerate() {
        let (batch_id, position) = row.map_err(sqlite_error)?;
        let position = from_sql_integer(position, "effect batch position")?;
        if position != expected_position as u64 {
            return Err(KernelError::Corrupt(
                "effect batch positions are not gapless".to_owned(),
            ));
        }
        let batch_id = parse_effect_batch_id(&batch_id)?;
        batches.push(EffectBatchSnapshot {
            batch_id,
            position,
            effects: load_effect_snapshots(connection, batch_id)?,
        });
    }
    Ok(batches)
}

pub(crate) fn load_settled_effect_batch(
    connection: &rusqlite::Connection,
    operation_id: OperationId,
    batch_id: EffectBatchId,
) -> Result<SettledEffectBatch, KernelError> {
    let facts = load_effect_batch_facts(connection, operation_id, batch_id, None)?;
    let mut effects = Vec::with_capacity(facts.effects.len());
    for fact in facts.effects {
        match fact {
            EffectFact::Settled(effect) => effects.push(effect),
            EffectFact::NotDispatched(_) | EffectFact::OutcomeUnknown(_) => {
                return Err(KernelError::Corrupt(
                    "settled input batch contains an unsettled child".to_owned(),
                ));
            }
        }
    }
    Ok(SettledEffectBatch { batch_id, effects })
}

pub(crate) fn load_effect_batch_facts(
    connection: &rusqlite::Connection,
    operation_id: OperationId,
    batch_id: EffectBatchId,
    manifest: Option<&RuntimeManifest>,
) -> Result<EffectBatchFacts, KernelError> {
    let owned = connection
        .query_row(
            "SELECT 1 FROM effect_batches WHERE batch_id = ?1 AND operation_id = ?2",
            params![batch_id.to_string(), operation_id.to_string()],
            |_| Ok(()),
        )
        .optional()
        .map_err(sqlite_error)?
        .is_some();
    if !owned {
        return Err(KernelError::Corrupt(
            "effect batch does not belong to the operation".to_owned(),
        ));
    }
    let mut statement = connection
        .prepare(
            "SELECT effect_id, position, binding, binding_revision, request_json,
                    status, outcome_json
             FROM effects WHERE batch_id = ?1 ORDER BY position",
        )
        .map_err(sqlite_error)?;
    let rows = statement
        .query_map([batch_id.to_string()], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, i64>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, String>(4)?,
                row.get::<_, String>(5)?,
                row.get::<_, Option<String>>(6)?,
            ))
        })
        .map_err(sqlite_error)?;
    let mut effects = Vec::new();
    for (expected_position, row) in rows.enumerate() {
        let (effect_id, position, binding, revision, request, status, outcome) =
            row.map_err(sqlite_error)?;
        if from_sql_integer(position, "effect position")? != expected_position as u64 {
            return Err(KernelError::Corrupt(
                "effect batch child positions are not gapless".to_owned(),
            ));
        }
        if let Some(manifest) = manifest
            && manifest.effect_bindings.get(&binding) != Some(&revision)
        {
            return Err(KernelError::Corrupt(
                "effect batch child differs from the frozen manifest".to_owned(),
            ));
        }
        let effect_id = parse_effect_id(&effect_id)?;
        let request: serde_json::Value = serde_json::from_str(&request).map_err(json_error)?;
        let unsettled = || UnsettledEffect {
            effect_id,
            binding: binding.clone(),
            binding_revision: revision.clone(),
            request: request.clone(),
        };
        let fact = match (parse_status(&status)?, outcome) {
            (EffectStatus::IntentCommitted, None) => EffectFact::NotDispatched(unsettled()),
            (EffectStatus::DispatchStarted | EffectStatus::OutcomeUnknown, None) => {
                EffectFact::OutcomeUnknown(unsettled())
            }
            (EffectStatus::Settled, Some(outcome)) => EffectFact::Settled(SettledEffect {
                effect_id,
                binding,
                binding_revision: revision,
                request,
                outcome: serde_json::from_str::<EffectOutcome>(&outcome).map_err(json_error)?,
            }),
            (EffectStatus::Settled, None) => {
                return Err(KernelError::Corrupt(
                    "settled effect has no outcome".to_owned(),
                ));
            }
            (_, Some(_)) => {
                return Err(KernelError::Corrupt(
                    "unsettled effect contains an outcome".to_owned(),
                ));
            }
        };
        effects.push(fact);
    }
    if effects.is_empty() {
        return Err(KernelError::Corrupt(
            "effect batch contains no children".to_owned(),
        ));
    }
    Ok(EffectBatchFacts { batch_id, effects })
}

fn load_effect_snapshots(
    connection: &rusqlite::Connection,
    batch_id: EffectBatchId,
) -> Result<Vec<EffectSnapshot>, KernelError> {
    let mut statement = connection
        .prepare(
            "SELECT effect_id, position, binding, binding_revision, recovery,
                    request_json, status, dispatch_count, outcome_json
             FROM effects WHERE batch_id = ?1 ORDER BY position",
        )
        .map_err(sqlite_error)?;
    let rows = statement
        .query_map([batch_id.to_string()], |row| {
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
    for (expected_position, row) in rows.enumerate() {
        let (id, position, binding, revision, recovery, request, status, dispatches, outcome) =
            row.map_err(sqlite_error)?;
        let position = from_sql_integer(position, "effect position")?;
        if position != expected_position as u64 {
            return Err(KernelError::Corrupt(
                "effect batch child positions are not gapless".to_owned(),
            ));
        }
        effects.push(EffectSnapshot {
            effect_id: parse_effect_id(&id)?,
            position,
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
    if effects.is_empty() {
        return Err(KernelError::Corrupt(
            "effect batch contains no children".to_owned(),
        ));
    }
    Ok(effects)
}
