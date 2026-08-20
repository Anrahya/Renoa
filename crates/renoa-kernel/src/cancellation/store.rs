use rusqlite::{OptionalExtension, params};
use serde_json::Value;

use super::{CancellationEffect, UnsettledEffect};
use crate::{
    EffectId, EffectOutcome, KernelError, OperationId, RuntimeManifest, SettledEffect,
    operation_phase::OperationPhase,
    schema::{json_error, sqlite_error},
};

pub(super) fn load_cancellation_effect(
    connection: &rusqlite::Connection,
    operation_id: OperationId,
    phase: OperationPhase,
    current_effect_id: Option<EffectId>,
    input_effect_id: Option<EffectId>,
    manifest: &RuntimeManifest,
) -> Result<Option<CancellationEffect>, KernelError> {
    let (effect_id, expected_status, kind) = match phase {
        OperationPhase::NeedDecision => {
            let Some(effect_id) = input_effect_id else {
                if current_effect_id.is_some() {
                    return Err(KernelError::Corrupt(
                        "decision phase contains a current effect".to_owned(),
                    ));
                }
                return Ok(None);
            };
            (effect_id, "settled", EffectKind::Settled)
        }
        OperationPhase::EffectIntent => (
            require_effect_id(current_effect_id, input_effect_id)?,
            "intent_committed",
            EffectKind::NotDispatched,
        ),
        OperationPhase::OutcomeUnknown => (
            require_effect_id(current_effect_id, input_effect_id)?,
            "outcome_unknown",
            EffectKind::OutcomeUnknown,
        ),
        _ => {
            return Err(KernelError::Corrupt(format!(
                "phase `{}` cannot be closed by cancellation",
                phase.as_str()
            )));
        }
    };
    let (binding, revision, request, status, outcome) = connection
        .query_row(
            "SELECT binding, binding_revision, request_json, status, outcome_json
             FROM effects WHERE operation_id = ?1 AND effect_id = ?2",
            params![operation_id.to_string(), effect_id.to_string()],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, Option<String>>(4)?,
                ))
            },
        )
        .optional()
        .map_err(sqlite_error)?
        .ok_or_else(|| KernelError::Corrupt("cancellation effect is missing".to_owned()))?;
    if status != expected_status || manifest.effect_bindings.get(&binding) != Some(&revision) {
        return Err(KernelError::Corrupt(
            "cancellation effect differs from durable operation state".to_owned(),
        ));
    }
    let request: Value = serde_json::from_str(&request).map_err(json_error)?;
    let unsettled = || UnsettledEffect {
        effect_id,
        binding: binding.clone(),
        binding_revision: revision.clone(),
        request: request.clone(),
    };
    match kind {
        EffectKind::NotDispatched if outcome.is_none() => {
            Ok(Some(CancellationEffect::NotDispatched(unsettled())))
        }
        EffectKind::OutcomeUnknown if outcome.is_none() => {
            Ok(Some(CancellationEffect::OutcomeUnknown(unsettled())))
        }
        EffectKind::Settled => {
            let outcome = outcome
                .map(|value| serde_json::from_str::<EffectOutcome>(&value).map_err(json_error))
                .transpose()?
                .ok_or_else(|| KernelError::Corrupt("settled effect has no outcome".to_owned()))?;
            Ok(Some(CancellationEffect::Settled(SettledEffect {
                effect_id,
                binding,
                binding_revision: revision,
                request,
                outcome,
            })))
        }
        EffectKind::NotDispatched | EffectKind::OutcomeUnknown => Err(KernelError::Corrupt(
            "unsettled cancellation effect contains an outcome".to_owned(),
        )),
    }
}

#[derive(Clone, Copy)]
enum EffectKind {
    NotDispatched,
    Settled,
    OutcomeUnknown,
}

fn require_effect_id(
    current_effect_id: Option<EffectId>,
    input_effect_id: Option<EffectId>,
) -> Result<EffectId, KernelError> {
    if input_effect_id.is_some() {
        return Err(KernelError::Corrupt(
            "active effect phase contains settled input".to_owned(),
        ));
    }
    current_effect_id
        .ok_or_else(|| KernelError::Corrupt("active effect identity is missing".to_owned()))
}
