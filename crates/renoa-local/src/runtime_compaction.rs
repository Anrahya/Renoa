use std::num::{NonZeroU32, NonZeroU64};

use renoa_agent_loop::CompactionLimits;

use crate::{AutomaticCompaction, BridgeModel, LocalRuntimeError};

pub(crate) const MAX_OUTPUT_TOKENS: NonZeroU32 = NonZeroU32::new(65_536).unwrap();
pub(crate) const MAX_CHECKPOINT_TOKENS: u64 = 16_384;
pub(crate) const MIN_CONTEXT_SAFETY_TOKENS: u64 = 8_192;
pub(crate) const REQUEST_DISPATCH_LIMIT_BYTES: NonZeroU64 =
    NonZeroU64::new(20 * 1_024 * 1_024).unwrap();
pub(crate) const AUTOMATIC_COMPACTION_BYTES: NonZeroU64 =
    NonZeroU64::new(8 * 1_024 * 1_024).unwrap();
pub(crate) const COMPACTION_TAIL_BYTES: NonZeroU64 = NonZeroU64::new(3 * 1_024 * 1_024).unwrap();

pub(crate) struct CompactionSettings {
    pub(crate) context: NonZeroU64,
    pub(crate) reserved: u64,
    pub(crate) automatic_compaction: NonZeroU64,
    pub(crate) target: NonZeroU64,
    pub(crate) max_summary: NonZeroU64,
}

const fn context_safety_tokens(provider_context: NonZeroU64) -> u64 {
    let proportional = provider_context.get() / 50;
    if proportional > MIN_CONTEXT_SAFETY_TOKENS {
        proportional
    } else {
        MIN_CONTEXT_SAFETY_TOKENS
    }
}

/// Leaves the safety margin and at least half of the remaining context window
/// for input, so raising the Host output cap never locks out a smaller-window
/// model whose provider advertises a near-window output limit.
pub(crate) fn effective_output_cap(
    provider_context: NonZeroU64,
    model_max_output: NonZeroU32,
) -> NonZeroU32 {
    let available = provider_context
        .get()
        .saturating_sub(context_safety_tokens(provider_context));
    let half_available = u32::try_from(available / 2).unwrap_or(u32::MAX);
    NonZeroU32::new(half_available).map_or(model_max_output, |budget| model_max_output.min(budget))
}

pub(crate) fn cap_model_output(model: BridgeModel) -> BridgeModel {
    let cap = effective_output_cap(model.context_window_tokens(), model.max_output_tokens());
    model.with_max_output_tokens(cap)
}

pub(crate) fn compaction_limits(
    provider_context: NonZeroU64,
    max_output: NonZeroU32,
    automatic_compaction: Option<AutomaticCompaction>,
    working_input_limit: Option<NonZeroU64>,
) -> Result<(CompactionSettings, CompactionLimits), LocalRuntimeError> {
    let settings = compaction_settings(
        provider_context,
        max_output,
        automatic_compaction,
        working_input_limit,
    )?;
    let limits = CompactionLimits::new(
        settings.context,
        settings.reserved,
        settings.target,
        settings.max_summary,
    )?
    .with_automatic_compaction_input_tokens(settings.automatic_compaction)?
    .with_request_byte_budget(
        REQUEST_DISPATCH_LIMIT_BYTES,
        AUTOMATIC_COMPACTION_BYTES,
        COMPACTION_TAIL_BYTES,
    )?;
    Ok((settings, limits))
}

pub(crate) fn compaction_settings(
    provider_context: NonZeroU64,
    max_output: NonZeroU32,
    automatic_compaction: Option<AutomaticCompaction>,
    working_input_limit: Option<NonZeroU64>,
) -> Result<CompactionSettings, LocalRuntimeError> {
    let safety = context_safety_tokens(provider_context);
    let reserved = u64::from(max_output.get())
        .checked_add(safety)
        .ok_or(LocalRuntimeError::ContextReserveOverflow)?;
    let context = working_input_limit.map_or(provider_context, |limit| {
        NonZeroU64::new(
            provider_context
                .get()
                .min(limit.get().saturating_add(reserved)),
        )
        .expect("positive context and working input limit")
    });
    let dispatch = context
        .get()
        .checked_sub(reserved)
        .and_then(NonZeroU64::new)
        .ok_or(LocalRuntimeError::ContextWindowTooSmall)?;
    // An agent's policy is written without knowing its model, so a trigger past
    // this model's dispatch limit, or a working limit's, compacts at the limit
    // instead, with the target kept to three fifths of it. A policy that fits
    // is used as stored.
    let automatic_compaction = match automatic_compaction {
        Some(policy) if policy.trigger_input_tokens > dispatch || working_input_limit.is_some() => {
            let trigger = policy.trigger_input_tokens.min(dispatch);
            let target = trigger
                .get()
                .checked_mul(3)
                .and_then(|value| NonZeroU64::new(value / 5))
                .ok_or(LocalRuntimeError::ZeroCompactionTarget)?;
            Some(AutomaticCompaction {
                trigger_input_tokens: trigger,
                target_input_tokens: policy.target_input_tokens.min(target),
            })
        }
        policy => policy,
    };
    let automatic_compaction_input_tokens =
        automatic_compaction.map_or(dispatch, |policy| policy.trigger_input_tokens);
    let target = match automatic_compaction {
        Some(policy) => policy.target_input_tokens,
        None => dispatch
            .get()
            .checked_mul(3)
            .and_then(|value| value.checked_div(5))
            .and_then(NonZeroU64::new)
            .ok_or(LocalRuntimeError::ZeroCompactionTarget)?,
    };
    let max_summary = NonZeroU64::new(MAX_CHECKPOINT_TOKENS.min(target.get() / 4))
        .ok_or(LocalRuntimeError::ZeroCheckpointBudget)?;
    Ok(CompactionSettings {
        context,
        reserved,
        automatic_compaction: automatic_compaction_input_tokens,
        target,
        max_summary,
    })
}

#[cfg(test)]
#[path = "runtime_compaction_tests.rs"]
mod compaction_settings_tests;
