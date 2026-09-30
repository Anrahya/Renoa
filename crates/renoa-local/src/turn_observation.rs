use std::time::{SystemTime, SystemTimeError, UNIX_EPOCH};

use thiserror::Error;

/// Wall-clock instant at which a Host admitted one user message.
///
/// Surfaces with durable inboxes should construct this from their persisted
/// receive time. Direct callers may use [`Self::now`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TurnObservation {
    unix_milliseconds: i64,
}

impl TurnObservation {
    /// Reads the Host clock once.
    ///
    /// # Errors
    ///
    /// Fails if the system clock precedes the Unix epoch or does not fit the
    /// supported signed-millisecond range.
    pub fn now() -> Result<Self, TurnObservationError> {
        let elapsed = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(TurnObservationError::Clock)?;
        let unix_milliseconds =
            i64::try_from(elapsed.as_millis()).map_err(|_| TurnObservationError::OutOfRange)?;
        Ok(Self { unix_milliseconds })
    }

    /// Restores a persisted surface receive time.
    ///
    /// # Errors
    ///
    /// Rejects values before the Unix epoch.
    pub const fn from_unix_milliseconds(
        unix_milliseconds: i64,
    ) -> Result<Self, TurnObservationError> {
        if unix_milliseconds < 0 {
            return Err(TurnObservationError::BeforeUnixEpoch);
        }
        Ok(Self { unix_milliseconds })
    }

    #[must_use]
    pub const fn unix_milliseconds(self) -> i64 {
        self.unix_milliseconds
    }
}

/// Invalid or unrepresentable Host turn time.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum TurnObservationError {
    #[error("Host clock precedes the Unix epoch: {0}")]
    Clock(#[source] SystemTimeError),
    #[error("Host clock does not fit the supported millisecond range")]
    OutOfRange,
    #[error("surface receive time cannot precede the Unix epoch")]
    BeforeUnixEpoch,
}

#[cfg(test)]
mod tests {
    use super::TurnObservation;

    #[test]
    fn persisted_observation_rejects_pre_epoch_time() {
        assert!(TurnObservation::from_unix_milliseconds(-1).is_err());
        assert_eq!(
            TurnObservation::from_unix_milliseconds(0)
                .expect("epoch")
                .unix_milliseconds(),
            0
        );
    }
}
