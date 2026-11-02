//! The log of a row the schedule passes over: once per row and error in a
//! process, then at most every [`REPEAT_AFTER`] with how often it was met.

use std::{
    collections::{HashMap, hash_map::Entry},
    sync::{LazyLock, Mutex, PoisonError},
    time::{Duration, Instant},
};

/// How long the same row and error stay quiet after they were logged.
const REPEAT_AFTER: Duration = Duration::from_secs(600);

static LOGGED: LazyLock<Mutex<Throttle>> = LazyLock::new(Mutex::default);

/// Logs `automation_unschedulable` for the row whose `field` is `id`, unless
/// the same row and error were logged less than [`REPEAT_AFTER`] ago.
pub(in crate::host::automations) fn report(field: &'static str, id: &str, error: &str) {
    let met = LOGGED
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .met(id, error, Instant::now());
    if let Some(met) = met {
        renoa_telemetry::event(
            "renoa.host",
            "warn",
            "automation_unschedulable",
            &serde_json::json!({ (field): id, "error": error, "met": met }),
        );
    }
}

/// When each key and error was last logged, and how often each was met
/// since: once per key and error in a process, then at most every
/// [`REPEAT_AFTER`]. Other logs that repeat each tick share it.
#[derive(Default)]
pub(crate) struct Throttle(HashMap<(String, String), Seen>);

struct Seen {
    logged_at: Instant,
    unlogged: u64,
}

impl Throttle {
    /// Counts `id` and `error` met at `now`. Returns how often they were met
    /// since last logged, this time included, when they are to be logged.
    pub(crate) fn met(&mut self, id: &str, error: &str, now: Instant) -> Option<u64> {
        let fresh = Seen {
            logged_at: now,
            unlogged: 0,
        };
        match self.0.entry((id.to_owned(), error.to_owned())) {
            Entry::Vacant(entry) => {
                entry.insert(fresh);
                Some(1)
            }
            Entry::Occupied(mut entry) => {
                let seen = entry.get_mut();
                seen.unlogged = seen.unlogged.saturating_add(1);
                if now.duration_since(seen.logged_at) < REPEAT_AFTER {
                    return None;
                }
                let met = seen.unlogged;
                *seen = fresh;
                Some(met)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, Instant};

    use super::{REPEAT_AFTER, Throttle};

    #[test]
    fn a_row_and_error_are_logged_once_then_every_ten_minutes_with_a_count() {
        let mut throttle = Throttle::default();
        let start = Instant::now();
        let at = |seconds| start + Duration::from_secs(seconds);
        assert_eq!(throttle.met("a", "bad", start), Some(1), "first time");
        assert_eq!(throttle.met("a", "bad", at(1)), None);
        assert_eq!(throttle.met("a", "other", at(1)), Some(1), "a new error");
        assert_eq!(throttle.met("b", "bad", at(1)), Some(1), "a new row");
        assert_eq!(throttle.met("a", "bad", at(599)), None);
        assert_eq!(
            throttle.met("a", "bad", start + REPEAT_AFTER),
            Some(3),
            "ten minutes on, with every time met since"
        );
        assert_eq!(throttle.met("a", "bad", at(601)), None, "quiet again");
    }
}
