use super::{RoutineError, RoutineSchedule, RoutineSpec};
use jiff::{Timestamp, ToSpan as _, civil::Time, tz::TimeZone};

impl RoutineSpec {
    pub(super) fn validate(&self, now_ms: i64) -> Result<(), RoutineError> {
        if self.name.trim().is_empty()
            || self.name.len() > 512
            || self.prompt.trim().is_empty()
            || self.prompt.len() > 32768
        {
            return Err(RoutineError::Invalid(
                "name and standing task must be nonempty and bounded".to_owned(),
            ));
        }
        self.schedule.next_after(now_ms)?;
        Ok(())
    }
}

impl RoutineSchedule {
    pub(super) fn first_due(&self, now_ms: i64, enabled: bool) -> Result<i64, RoutineError> {
        let due = self.next_after(now_ms)?;
        if enabled && matches!(self, Self::Once { .. }) && due <= now_ms {
            return Err(RoutineError::Invalid(
                "one-time schedules must be in the future when armed".to_owned(),
            ));
        }
        Ok(due)
    }

    pub(super) fn advance_past(&self, due_ms: i64, now_ms: i64) -> Result<i64, RoutineError> {
        if let Self::Interval { hours } = self {
            let period = i64::from(*hours) * 3_600_000;
            self.next_after(now_ms)?;
            let missed = now_ms
                .checked_sub(due_ms)
                .and_then(|elapsed| elapsed.checked_div(period))
                .and_then(|count| count.checked_add(1));
            return missed
                .and_then(|count| count.checked_mul(period))
                .and_then(|delta| due_ms.checked_add(delta))
                .ok_or_else(|| RoutineError::Invalid("schedule time overflow".to_owned()));
        }
        self.next_after(now_ms)
    }

    pub(super) fn next_after(&self, now_ms: i64) -> Result<i64, RoutineError> {
        if now_ms < 0 {
            return Err(RoutineError::Invalid(
                "time must follow the Unix epoch".to_owned(),
            ));
        }
        match self {
            Self::Once { at } => {
                if at.len() > 128 {
                    return Err(RoutineError::Invalid(
                        "one-time timestamp is too long".to_owned(),
                    ));
                }
                let due = at.parse::<Timestamp>()?.as_millisecond();
                if due < 0 {
                    return Err(RoutineError::Invalid(
                        "one-time schedule must follow the Unix epoch".to_owned(),
                    ));
                }
                Ok(due)
            }
            Self::Interval { hours } => {
                if !(1..=8760).contains(hours) {
                    return Err(RoutineError::Invalid(
                        "interval must be 1–8760 hours".to_owned(),
                    ));
                }
                now_ms
                    .checked_add(i64::from(*hours) * 3_600_000)
                    .ok_or_else(|| RoutineError::Invalid("schedule time overflow".to_owned()))
            }
            Self::Daily {
                hour,
                minute,
                timezone,
            } => {
                let time = Time::new(*hour, *minute, 0, 0)?;
                let zone = TimeZone::get(timezone)?;
                let now = Timestamp::from_millisecond(now_ms)?.to_zoned(zone.clone());
                let mut date = now.date();
                // Compatible resolution chooses the first repeated time in fall
                // and shifts a nonexistent spring time forward across the gap.
                let mut next = date.to_datetime(time).to_zoned(zone.clone())?.timestamp();
                if next.as_millisecond() <= now_ms {
                    date = date.checked_add(1.day())?;
                    next = date.to_datetime(time).to_zoned(zone)?.timestamp();
                }
                Ok(next.as_millisecond())
            }
        }
    }
}
