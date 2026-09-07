use renoa_agent::TokenUsage;
use renoa_kernel::{EffectOutcome, SessionSnapshot};

use crate::{configuration::MODEL_EFFECT_BINDING, format::ModelEffectOutput};

/// Totals recorded model responses, including summary attempts. Unknown outcomes,
/// missing provider accounting or arithmetic overflow make the total unknown.
#[must_use]
pub fn recorded_token_usage(snapshot: &SessionSnapshot) -> Option<TokenUsage> {
    let mut total = TokenUsage::default();
    let mut observed = false;
    for effect in snapshot
        .operations
        .iter()
        .flat_map(|operation| &operation.effects)
        .filter(|effect| effect.binding == MODEL_EFFECT_BINDING)
    {
        let Some(EffectOutcome::Success(value)) = &effect.outcome else {
            return None;
        };
        let output: ModelEffectOutput = serde_json::from_value(value.clone()).ok()?;
        let ModelEffectOutput::Completed { response } = output else {
            continue;
        };
        let usage = response.usage?;
        observed = true;
        total.input = total.input.checked_add(usage.input)?;
        total.output = total.output.checked_add(usage.output)?;
        total.cache_read = total.cache_read.checked_add(usage.cache_read)?;
        total.cache_write = total.cache_write.checked_add(usage.cache_write)?;
    }
    observed.then_some(total)
}
