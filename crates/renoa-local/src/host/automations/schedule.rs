use super::{AutomationError, AutomationSchedule, AutomationSpec, cron::Cron};
use jiff::{Timestamp, tz::TimeZone};

impl AutomationSpec {
    pub(super) fn validate(&self, now_ms: i64) -> Result<(), AutomationError> {
        if self.name.trim().is_empty()
            || self.name.len() > 512
            || self.prompt.trim().is_empty()
            || self.prompt.len() > 32768
        {
            return Err(AutomationError::Invalid(
                "name and standing task must be nonempty and bounded".to_owned(),
            ));
        }
        self.schedule.next_after(now_ms)?;
        Ok(())
    }
}

impl AutomationSchedule {
    pub(super) fn first_due(&self, now_ms: i64, enabled: bool) -> Result<i64, AutomationError> {
        let due = self.next_after(now_ms)?;
        if enabled && matches!(self, Self::Once { .. }) && due <= now_ms {
            return Err(AutomationError::Invalid(
                "one-time schedules must be in the future when armed".to_owned(),
            ));
        }
        Ok(due)
    }

    /// How late a run due at `due_ms` may start before it is skipped: half the
    /// gap to the schedule's next run, so a late run never lands nearer the
    /// next occurrence than its own. A one-time run always runs, however late.
    pub(super) fn skip_after_ms(&self, due_ms: i64) -> Result<Option<i64>, AutomationError> {
        match self {
            Self::Once { .. } => Ok(None),
            Self::Cron { .. } => Ok(Some((self.next_after(due_ms)? - due_ms) / 2)),
        }
    }

    /// A run's due time as its context line shows it: the local hour and
    /// minute with the zone's abbreviation. A one-time run's time is in its
    /// own task, so it has none.
    pub(super) fn due_label(&self, due_ms: i64) -> Result<Option<String>, AutomationError> {
        match self {
            Self::Once { .. } => Ok(None),
            Self::Cron { timezone, .. } => Ok(Some(
                Timestamp::from_millisecond(due_ms)?
                    .to_zoned(zone(timezone)?)
                    .strftime("%H:%M %Z")
                    .to_string(),
            )),
        }
    }

    pub(super) fn next_after(&self, now_ms: i64) -> Result<i64, AutomationError> {
        if now_ms < 0 {
            return Err(AutomationError::Invalid(
                "time must follow the Unix epoch".to_owned(),
            ));
        }
        match self {
            Self::Once { at } => {
                if at.len() > 128 {
                    return Err(AutomationError::Invalid(
                        "one-time timestamp is too long".to_owned(),
                    ));
                }
                let due = at.parse::<Timestamp>()?.as_millisecond();
                if due < 0 {
                    return Err(AutomationError::Invalid(
                        "one-time schedule must follow the Unix epoch".to_owned(),
                    ));
                }
                Ok(due)
            }
            Self::Cron {
                expression,
                timezone,
            } => Ok(Cron::parse(expression)?
                .next_after(&zone(timezone)?, Timestamp::from_millisecond(now_ms)?)?
                .as_millisecond()),
        }
    }
}

fn zone(name: &str) -> Result<TimeZone, AutomationError> {
    TimeZone::get(name).map_err(|_| {
        AutomationError::Invalid(format!(
            "timezone \"{name}\" is not an IANA timezone name; use one like \"Asia/Kolkata\" or \"America/New_York\""
        ))
    })
}
