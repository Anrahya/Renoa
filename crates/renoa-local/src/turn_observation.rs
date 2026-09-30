use std::time::{SystemTime, SystemTimeError, UNIX_EPOCH};

use thiserror::Error;

/// When a Host admitted one user message and, if its surface said so, where
/// the message was written.
///
/// Surfaces with durable inboxes should construct this from their persisted
/// receive time. Direct callers may use [`Self::now`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TurnObservation {
    unix_milliseconds: i64,
    surface_context: Option<String>,
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
        Ok(Self {
            unix_milliseconds,
            surface_context: None,
        })
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
        Ok(Self {
            unix_milliseconds,
            surface_context: None,
        })
    }

    /// Adds the surface's own description of where the message was written.
    /// A new command admits it as the `surface` context entry; one that does
    /// not fit that entry's bounds is left out and traced.
    #[must_use]
    pub fn with_surface_context(mut self, context: impl Into<String>) -> Self {
        self.surface_context = Some(context.into());
        self
    }

    #[must_use]
    pub const fn unix_milliseconds(&self) -> i64 {
        self.unix_milliseconds
    }

    #[must_use]
    pub fn surface_context(&self) -> Option<&str> {
        self.surface_context.as_deref()
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
