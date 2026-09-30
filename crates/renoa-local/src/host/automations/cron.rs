//! Five-field cron expressions, evaluated in an IANA timezone.
//!
//! The fields are minute, hour, day of month, month and weekday. Each accepts
//! `*`, a value, a range `a-b`, a step `/n` after either, and comma lists.
//! Months and weekdays also accept three-letter English names, and weekday 7
//! is Sunday like 0. When both day of month and weekday are restricted, a day
//! matching either runs, as in standard cron; a field starting with `*` is
//! unrestricted.

use jiff::{
    Timestamp, ToSpan as _,
    civil::{Date, Time},
    tz::TimeZone,
};

use super::AutomationError;

/// The least time, in minutes, a cron schedule leaves between two runs.
const MIN_GAP_MINUTES: u32 = 5;
/// The longest a valid expression can wait: 29 February recurs within eight
/// years.
const SEARCH_DAYS: u32 = 8 * 366 + 1;

struct Field {
    name: &'static str,
    min: u32,
    max: u32,
    /// Names for the values from `min`, in order.
    names: &'static [&'static str],
    accepts: &'static str,
}

const MINUTE: Field = Field {
    name: "minute",
    min: 0,
    max: 59,
    names: &[],
    accepts: "0-59",
};
const HOUR: Field = Field {
    name: "hour",
    min: 0,
    max: 23,
    names: &[],
    accepts: "0-23",
};
const DAY: Field = Field {
    name: "day-of-month",
    min: 1,
    max: 31,
    names: &[],
    accepts: "1-31",
};
const MONTH: Field = Field {
    name: "month",
    min: 1,
    max: 12,
    names: &[
        "JAN", "FEB", "MAR", "APR", "MAY", "JUN", "JUL", "AUG", "SEP", "OCT", "NOV", "DEC",
    ],
    accepts: "1-12 or JAN-DEC",
};
const WEEKDAY: Field = Field {
    name: "weekday",
    min: 0,
    max: 7,
    names: &["SUN", "MON", "TUE", "WED", "THU", "FRI", "SAT"],
    accepts: "0-7 or SUN-SAT, where 0 and 7 are Sunday",
};

/// A parsed expression. Each field is a bit set of the values it allows.
pub(super) struct Cron {
    minutes: u64,
    hours: u64,
    days: u64,
    months: u64,
    /// Bit 0 is Sunday.
    weekdays: u64,
    days_restricted: bool,
    weekdays_restricted: bool,
}

fn invalid(message: String) -> AutomationError {
    AutomationError::Invalid(message)
}

impl Cron {
    /// Parses an expression and refuses one that runs less than
    /// [`MIN_GAP_MINUTES`] apart. The errors say how to fix the call.
    pub(super) fn parse(expression: &str) -> Result<Self, AutomationError> {
        if expression.len() > 128 {
            return Err(invalid(
                "cron expression is too long; use at most 128 characters".to_owned(),
            ));
        }
        let fields = expression.split_whitespace().collect::<Vec<_>>();
        let [minute, hour, day, month, weekday] = fields[..] else {
            return Err(invalid(format!(
                "cron expression needs 5 fields \"minute hour day-of-month month weekday\", e.g. \"30 9 * * 1-5\" for 09:30 on weekdays; \"{expression}\" has {}",
                fields.len()
            )));
        };
        let weekdays = parse_field(weekday, &WEEKDAY)?;
        let cron = Self {
            minutes: parse_field(minute, &MINUTE)?,
            hours: parse_field(hour, &HOUR)?,
            days: parse_field(day, &DAY)?,
            months: parse_field(month, &MONTH)?,
            weekdays: (weekdays | (weekdays >> 7)) & 0x7f,
            days_restricted: !day.starts_with('*'),
            weekdays_restricted: !weekday.starts_with('*'),
        };
        // Runs within an hour are as far apart as its minutes, and runs in
        // different hours are at least as far apart as the wrap from the last
        // minute to the first, so the minute field alone bounds every gap.
        let minutes = values(cron.minutes, 60).collect::<Vec<_>>();
        let wrap = minutes.first().map(|first| first + 60);
        let gaps = minutes.iter().copied().chain(wrap).collect::<Vec<_>>();
        if gaps
            .windows(2)
            .any(|pair| pair[1] - pair[0] < MIN_GAP_MINUTES)
        {
            return Err(invalid(format!(
                "cron minute field \"{minute}\" runs less than {MIN_GAP_MINUTES} minutes apart; leave at least {MIN_GAP_MINUTES} minutes between runs, e.g. \"*/5\" or \"0,30\""
            )));
        }
        Ok(cron)
    }

    /// The first run strictly after `now`. A local time that a clock change
    /// skips runs at the same distance past the gap, and a repeated local
    /// time runs once, at its first occurrence.
    pub(super) fn next_after(
        &self,
        zone: &TimeZone,
        now: Timestamp,
    ) -> Result<Timestamp, AutomationError> {
        let mut date = now.to_zoned(zone.clone()).date();
        for _ in 0..SEARCH_DAYS {
            if self.runs_on(date) {
                let mut earliest = None::<Timestamp>;
                for hour in values(self.hours, 24) {
                    for minute in values(self.minutes, 60) {
                        let time = Time::new(clock(hour)?, clock(minute)?, 0, 0)?;
                        // A skipped time can shift past a later one, so the
                        // day's earliest run is the least, not the first.
                        let at = date.to_datetime(time).to_zoned(zone.clone())?.timestamp();
                        if at > now && earliest.is_none_or(|first| at < first) {
                            earliest = Some(at);
                        }
                    }
                }
                if let Some(at) = earliest {
                    return Ok(at);
                }
            }
            date = date.checked_add(1.day())?;
        }
        Err(invalid(
            "cron expression never runs; check that its day-of-month exists in its month"
                .to_owned(),
        ))
    }

    fn runs_on(&self, date: Date) -> bool {
        let has = |set: u64, value: i8| set & (1 << value.unsigned_abs()) != 0;
        if !has(self.months, date.month()) {
            return false;
        }
        let day = has(self.days, date.day());
        let weekday = has(self.weekdays, date.weekday().to_sunday_zero_offset());
        if self.days_restricted && self.weekdays_restricted {
            day || weekday
        } else {
            day && weekday
        }
    }
}

fn parse_field(text: &str, field: &Field) -> Result<u64, AutomationError> {
    let mut set = 0;
    for item in text.split(',') {
        let (range, step) = match item.split_once('/') {
            Some((range, step)) => (range, Some(step)),
            None => (item, None),
        };
        let (low, high) = if range == "*" {
            (field.min, field.max)
        } else if let Some((low, high)) = range.split_once('-') {
            (value(low, field)?, value(high, field)?)
        } else {
            let single = value(range, field)?;
            // `a/n` steps from a to the field's end.
            (single, if step.is_some() { field.max } else { single })
        };
        if low > high {
            return Err(invalid(format!(
                "cron {} range \"{range}\" runs backwards; write the smaller value first",
                field.name
            )));
        }
        let step = match step {
            None => 1,
            Some(step) => step
                .parse::<usize>()
                .ok()
                .filter(|step| *step > 0)
                .ok_or_else(|| {
                    invalid(format!(
                        "cron {} step \"{step}\" must be a whole number of at least 1",
                        field.name
                    ))
                })?,
        };
        for allowed in (low..=high).step_by(step) {
            set |= 1 << allowed;
        }
    }
    Ok(set)
}

fn value(text: &str, field: &Field) -> Result<u32, AutomationError> {
    let named = || {
        let index = field
            .names
            .iter()
            .position(|name| name.eq_ignore_ascii_case(text))?;
        u32::try_from(index).ok().map(|index| index + field.min)
    };
    match text.parse::<u32>().ok().or_else(named) {
        Some(value) if (field.min..=field.max).contains(&value) => Ok(value),
        _ => Err(invalid(format!(
            "cron {} value \"{text}\" must be {}",
            field.name, field.accepts
        ))),
    }
}

/// The values in `set` below `limit`, in ascending order.
fn values(set: u64, limit: u32) -> impl Iterator<Item = u32> {
    (0..limit).filter(move |value| set & (1 << value) != 0)
}

fn clock(value: u32) -> Result<i8, AutomationError> {
    i8::try_from(value).map_err(|_| invalid("cron time is out of range".to_owned()))
}
