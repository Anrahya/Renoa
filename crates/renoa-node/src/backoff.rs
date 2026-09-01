use std::time::Duration;

const INITIAL_DELAY: Duration = Duration::from_millis(250);
const MAX_DELAY: Duration = Duration::from_secs(30);
pub(crate) const STABLE_CONNECTION: Duration = Duration::from_secs(30);

pub(crate) struct ReconnectBackoff {
    failures: u32,
}

impl ReconnectBackoff {
    pub(crate) const fn new() -> Self {
        Self { failures: 0 }
    }

    pub(crate) fn next_delay(&mut self, stable_connection: bool) -> Duration {
        if stable_connection {
            self.failures = 0;
        }
        let exponent = self.failures.min(7);
        let multiplier = 1_u32 << exponent;
        self.failures = self.failures.saturating_add(1);
        INITIAL_DELAY.saturating_mul(multiplier).min(MAX_DELAY)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reconnects_back_off_boundedly_and_reset_after_stability() {
        let mut backoff = ReconnectBackoff::new();
        assert_eq!(backoff.next_delay(false), Duration::from_millis(250));
        assert_eq!(backoff.next_delay(false), Duration::from_millis(500));
        assert_eq!(backoff.next_delay(false), Duration::from_secs(1));
        for _ in 0..20 {
            assert!(backoff.next_delay(false) <= MAX_DELAY);
        }
        assert_eq!(backoff.next_delay(true), Duration::from_millis(250));
    }
}
