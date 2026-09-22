use crate::{
    EffectBatchFacts, EffectBatchId, EffectFact, KernelError, OperationId, RuntimeManifest,
    effect_store::load_effect_batch_facts, operation_phase::OperationPhase,
};

pub(super) fn load_cancellation_effect_batch(
    connection: &rusqlite::Connection,
    operation_id: OperationId,
    phase: OperationPhase,
    current_batch_id: Option<EffectBatchId>,
    input_batch_id: Option<EffectBatchId>,
    manifest: &RuntimeManifest,
) -> Result<Option<EffectBatchFacts>, KernelError> {
    let batch_id = match phase {
        OperationPhase::NeedDecision => {
            let Some(batch_id) = input_batch_id else {
                if current_batch_id.is_some() {
                    return Err(KernelError::Corrupt(
                        "decision phase contains a current effect batch".to_owned(),
                    ));
                }
                return Ok(None);
            };
            if current_batch_id.is_some() {
                return Err(KernelError::Corrupt(
                    "decision phase contains both current and input effect batches".to_owned(),
                ));
            }
            batch_id
        }
        OperationPhase::EffectIntent | OperationPhase::OutcomeUnknown => {
            if input_batch_id.is_some() {
                return Err(KernelError::Corrupt(
                    "active effect phase contains settled batch input".to_owned(),
                ));
            }
            current_batch_id.ok_or_else(|| {
                KernelError::Corrupt("active effect batch identity is missing".to_owned())
            })?
        }
        _ => {
            return Err(KernelError::Corrupt(format!(
                "phase `{}` cannot be closed by cancellation",
                phase.as_str()
            )));
        }
    };
    let facts = load_effect_batch_facts(connection, operation_id, batch_id, Some(manifest))?;
    let valid = match phase {
        OperationPhase::NeedDecision => facts
            .effects
            .iter()
            .all(|fact| matches!(fact, EffectFact::Settled(_))),
        OperationPhase::EffectIntent => facts
            .effects
            .iter()
            .all(|fact| matches!(fact, EffectFact::NotDispatched(_))),
        OperationPhase::OutcomeUnknown => {
            facts
                .effects
                .iter()
                .any(|fact| matches!(fact, EffectFact::OutcomeUnknown(_)))
                && facts.effects.iter().all(|fact| {
                    matches!(fact, EffectFact::Settled(_) | EffectFact::OutcomeUnknown(_))
                })
        }
        _ => false,
    };
    if !valid {
        return Err(KernelError::Corrupt(
            "cancellation effect batch differs from durable operation state".to_owned(),
        ));
    }
    Ok(Some(facts))
}
