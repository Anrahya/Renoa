use renoa_kernel::{LoopError, SettledEffect, SettledEffectBatch};

pub(super) fn require_effect(
    batch: Option<SettledEffectBatch>,
    expected: &str,
) -> Result<SettledEffect, LoopError> {
    let batch = batch
        .ok_or_else(|| LoopError::new(format!("checkpoint is missing its settled {expected}")))?;
    let mut effects = batch.effects.into_iter();
    let effect = effects
        .next()
        .ok_or_else(|| LoopError::new(format!("settled {expected} batch contains no effect")))?;
    if effects.next().is_some() {
        return Err(LoopError::new(format!(
            "settled {expected} batch contains more than one effect"
        )));
    }
    Ok(effect)
}

pub(super) fn require_effect_identity(
    effect: &SettledEffect,
    binding: &str,
    request: &serde_json::Value,
) -> Result<(), LoopError> {
    require_effect_request_identity(
        "settled",
        &effect.binding,
        &effect.request,
        binding,
        request,
    )
}

pub(super) fn require_effect_request_identity(
    kind: &str,
    actual_binding: &str,
    actual_request: &serde_json::Value,
    expected_binding: &str,
    expected_request: &serde_json::Value,
) -> Result<(), LoopError> {
    if actual_binding != expected_binding {
        return Err(LoopError::new(format!(
            "{kind} effect binding `{actual_binding}` differs from expected `{expected_binding}`"
        )));
    }
    if actual_request != expected_request {
        return Err(LoopError::new(format!(
            "{kind} effect request differs from durable loop state"
        )));
    }
    Ok(())
}
