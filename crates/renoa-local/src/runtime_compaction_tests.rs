use std::num::{NonZeroU32, NonZeroU64};

use super::{
    AUTOMATIC_COMPACTION_BYTES, COMPACTION_TAIL_BYTES, MAX_OUTPUT_TOKENS,
    REQUEST_DISPATCH_LIMIT_BYTES, compaction_limits, compaction_settings, effective_output_cap,
};
use crate::{AutomaticCompaction, ReasoningLevel, runtime::model_revision_for};

fn tokens(value: u64) -> NonZeroU64 {
    NonZeroU64::new(value).expect("non-zero")
}

const OUTPUT: NonZeroU32 = NonZeroU32::new(8_192).unwrap();

fn policy(trigger: u64, target: u64) -> AutomaticCompaction {
    AutomaticCompaction {
        trigger_input_tokens: tokens(trigger),
        target_input_tokens: tokens(target),
    }
}

#[test]
fn a_trigger_past_a_small_models_limit_compacts_at_the_limit() {
    // 100,000 less 8,192 of output and 8,192 of safety leaves 83,616.
    let settings =
        compaction_settings(tokens(100_000), OUTPUT, Some(policy(450_000, 60_000)), None)
            .expect("settings");
    assert_eq!(settings.automatic_compaction.get(), 83_616);
    assert_eq!(
        settings.target.get(),
        50_169,
        "three fifths of the limit, below the stored 60,000"
    );
}

#[test]
fn a_policy_that_fits_is_used_as_stored() {
    let settings = compaction_settings(
        tokens(1_000_000),
        OUTPUT,
        Some(policy(450_000, 400_000)),
        None,
    )
    .expect("settings");
    assert_eq!(settings.automatic_compaction.get(), 450_000);
    assert_eq!(
        settings.target.get(),
        400_000,
        "a target the model can take is not cut to three fifths"
    );
}

#[test]
fn runtime_compaction_limits_bind_the_request_byte_budget() {
    let (_, limits) = compaction_limits(tokens(1_000_000), OUTPUT, None, None).expect("limits");
    let budget = limits
        .request_byte_budget()
        .expect("runtime must bind a byte budget");
    assert_eq!(budget.dispatch_limit_bytes(), REQUEST_DISPATCH_LIMIT_BYTES);
    assert_eq!(
        budget.automatic_compaction_bytes(),
        AUTOMATIC_COMPACTION_BYTES
    );
    assert_eq!(budget.tail_budget_bytes(), COMPACTION_TAIL_BYTES);
}

#[test]
fn host_output_cap_is_64k_and_preserves_400k_compaction_trigger() {
    assert_eq!(MAX_OUTPUT_TOKENS.get(), 65_536);
    let settings = compaction_settings(
        tokens(1_000_000),
        MAX_OUTPUT_TOKENS,
        Some(policy(400_000, 40_000)),
        None,
    )
    .expect("settings");
    assert_eq!(settings.reserved, 65_536 + 20_000);
    assert_eq!(settings.automatic_compaction.get(), 400_000);
    assert_eq!(settings.target.get(), 40_000);
}

#[test]
fn small_window_models_clamp_output_cap_so_input_budget_stays_usable() {
    // OpenRouter MiniMax M2.5 and Gemini Nano Banana 2.1: 65,536 context with
    // a 58,982-token provider output limit.
    let provider_max = NonZeroU32::new(58_982).expect("non-zero");
    let effective = effective_output_cap(tokens(65_536), provider_max.min(MAX_OUTPUT_TOKENS));
    assert_eq!(effective.get(), 28_672);
    let settings = compaction_settings(tokens(65_536), effective, None, None)
        .expect("small-window model must resolve without ContextWindowTooSmall");
    assert_eq!(settings.reserved, 28_672 + 8_192);
    assert_eq!(settings.automatic_compaction.get(), 28_672);
}

#[test]
fn model_revision_changes_when_effective_output_cap_changes() {
    let old = model_revision_for(
        "xai",
        "grok-4.7",
        "binding",
        ReasoningLevel::High,
        NonZeroU32::new(32_768).expect("non-zero"),
    );
    let new = model_revision_for(
        "xai",
        "grok-4.7",
        "binding",
        ReasoningLevel::High,
        MAX_OUTPUT_TOKENS,
    );
    assert_ne!(old, new);
    assert!(new.ends_with("/max-output-65536"));
}
