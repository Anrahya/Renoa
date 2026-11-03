use std::num::{NonZeroU32, NonZeroU64};

use super::compaction_settings;
use crate::AutomaticCompaction;

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
