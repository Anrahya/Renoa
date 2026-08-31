use std::time::{SystemTime, SystemTimeError, UNIX_EPOCH};

use jiff::{Timestamp, fmt::strtime, tz::TimeZone};
use renoa_agent_loop::{TurnTiming, TurnTimingError};
use thiserror::Error;

const DISPLAY_FORMAT: &str = "%Y-%m-%dT%H:%M:%S%:z[%Q]";

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

    pub(crate) fn turn_timing(
        self,
        previous_unix_milliseconds: Option<i64>,
    ) -> Result<TurnTiming, TurnObservationError> {
        let time_zone = TimeZone::try_system().unwrap_or(TimeZone::UTC);
        self.turn_timing_in(previous_unix_milliseconds, time_zone)
    }

    fn turn_timing_in(
        self,
        previous_unix_milliseconds: Option<i64>,
        time_zone: TimeZone,
    ) -> Result<TurnTiming, TurnObservationError> {
        let timestamp = Timestamp::from_millisecond(self.unix_milliseconds)?;
        let observed_at = strtime::format(DISPLAY_FORMAT, &timestamp.to_zoned(time_zone))?;
        let elapsed = previous_unix_milliseconds
            .and_then(|previous| self.unix_milliseconds.checked_sub(previous))
            .and_then(|milliseconds| u64::try_from(milliseconds).ok());
        TurnTiming::new(observed_at, self.unix_milliseconds, elapsed).map_err(Into::into)
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
    #[error("Host turn time cannot be represented: {0}")]
    DateTime(#[from] jiff::Error),
    #[error(transparent)]
    TurnTiming(#[from] TurnTimingError),
}

#[cfg(test)]
mod tests {
    use jiff::tz::TimeZone;

    use super::TurnObservation;

    #[test]
    fn fixed_observation_formats_with_offset_zone_and_elapsed_time() {
        let observation =
            TurnObservation::from_unix_milliseconds(1_788_199_445_000).expect("valid observation");
        let timing = observation
            .turn_timing_in(
                Some(1_788_195_845_000),
                TimeZone::get("Asia/Kolkata").expect("known time zone"),
            )
            .expect("format timing");

        assert_eq!(timing.observed_at_unix_ms(), 1_788_199_445_000);
        assert_eq!(
            timing.elapsed_since_previous_user_message_ms(),
            Some(3_600_000)
        );
        assert!(timing.observed_at().ends_with("+05:30[Asia/Kolkata]"));
    }

    #[test]
    fn backward_clock_change_omits_elapsed_instead_of_inventing_a_duration() {
        let observation =
            TurnObservation::from_unix_milliseconds(1_000).expect("valid observation");
        let timing = observation
            .turn_timing_in(Some(2_000), TimeZone::UTC)
            .expect("format timing");

        assert_eq!(timing.elapsed_since_previous_user_message_ms(), None);
    }

    #[test]
    fn persisted_observation_rejects_pre_epoch_time() {
        assert!(TurnObservation::from_unix_milliseconds(-1).is_err());
    }
}
