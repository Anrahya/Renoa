//! When faces blink: every few seconds, each agent at its own moment, all
//! from one clock, so the window draws again only when an eye opens or shuts.

use std::time::Duration;

/// How often a face blinks, as the motion board has it.
const EVERY_MS: u64 = 4200;

/// How long the eyes stay shut.
const SHUT_MS: u64 = 170;

/// Where in the cycle the agent `seed` names blinks.
fn moment(seed: u128) -> u64 {
    u64::try_from(seed % u128::from(EVERY_MS)).unwrap_or(0)
}

/// How far into its own cycle the agent `seed` is at `now_ms`.
fn into_cycle(seed: u128, now_ms: u64) -> u64 {
    (now_ms + EVERY_MS - moment(seed)) % EVERY_MS
}

/// Whether the agent `seed` has its eyes shut at `now_ms`.
pub fn shut(seed: u128, now_ms: u64) -> bool {
    into_cycle(seed, now_ms) < SHUT_MS
}

/// How long from `now_ms` until any of `seeds` opens or shuts its eyes; a
/// whole cycle with none.
pub fn next_change(seeds: impl IntoIterator<Item = u128>, now_ms: u64) -> Duration {
    let wait = seeds
        .into_iter()
        .map(|seed| {
            let at = into_cycle(seed, now_ms);
            if at < SHUT_MS {
                SHUT_MS - at
            } else {
                EVERY_MS - at
            }
        })
        .min()
        .unwrap_or(EVERY_MS);
    Duration::from_millis(wait)
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::{EVERY_MS, SHUT_MS, next_change, shut};

    #[test]
    fn each_agent_blinks_once_a_cycle_at_its_own_moment() {
        let (early, late) = (100, 2100);
        let shut_at = |seed| {
            (0..EVERY_MS)
                .filter(|&now| shut(seed, now))
                .collect::<Vec<_>>()
        };
        assert_eq!(shut_at(early), (100..100 + SHUT_MS).collect::<Vec<_>>());
        assert_eq!(shut_at(late).first(), Some(&2100));
        assert!(shut(early, EVERY_MS + 100), "and again a cycle later");
    }

    #[test]
    fn the_clock_wakes_for_the_next_eye_to_open_or_shut() {
        let ms = Duration::from_millis;
        assert_eq!(next_change([100, 2100], 0), ms(100), "the first to shut");
        assert_eq!(
            next_change([100, 2100], 150),
            ms(SHUT_MS - 50),
            "the shut one opening"
        );
        assert_eq!(
            next_change([100, 2100], 300),
            ms(1800),
            "the second to shut"
        );
        assert_eq!(
            next_change([100], 100 + SHUT_MS),
            ms(EVERY_MS - SHUT_MS),
            "an eye just opened waits for its next blink"
        );
        assert_eq!(next_change([], 0), ms(EVERY_MS), "no faces, a quiet cycle");
    }
}
