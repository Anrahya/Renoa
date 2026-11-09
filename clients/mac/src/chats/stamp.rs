//! When lines were said, as the chats show it: a time between lines far
//! apart, and beside each agent the time of its chat's last timed line.

use chrono::{DateTime, Datelike as _, TimeZone};

/// Lines at least this far apart get a time between them.
const GAP_MS: i64 = 60 * 60 * 1000;

/// How many days back a day goes by its name rather than its date.
const NAMED_DAYS: i64 = 6;

/// Whether a time goes between a line said at `before` and one said at
/// `at`: at the first line with a time, and wherever an hour or a midnight
/// passed between them.
pub fn separates<Tz: TimeZone>(before: Option<i64>, at: Option<i64>, zone: &Tz) -> bool {
    let Some(at) = at else {
        return false;
    };
    let Some(before) = before else {
        return true;
    };
    at - before >= GAP_MS || day(before, zone) != day(at, zone)
}

/// The time between lines: "Today 6:12 PM", "Yesterday 9:03 AM",
/// "Monday 6:12 PM", "3 Nov 6:12 PM" or, another year, "3 Nov 2025 6:12 PM".
pub fn long<Tz: TimeZone>(at_ms: i64, now: &DateTime<Tz>) -> String
where
    Tz::Offset: std::fmt::Display,
{
    let Some(at) = local(at_ms, now) else {
        return String::new();
    };
    let clock = at.format("%-I:%M %p");
    match days_ago(&at, now) {
        0 => format!("Today {clock}"),
        1 => format!("Yesterday {clock}"),
        2..=NAMED_DAYS => format!("{} {clock}", at.format("%A")),
        _ if at.year() == now.year() => format!("{} {clock}", at.format("%-d %b")),
        _ => format!("{} {clock}", at.format("%-d %b %Y")),
    }
}

/// The time beside an agent: "6:12 PM", "Yesterday", "Monday", "3 Nov" or,
/// another year, "3 Nov 2025".
pub fn short<Tz: TimeZone>(at_ms: i64, now: &DateTime<Tz>) -> String
where
    Tz::Offset: std::fmt::Display,
{
    let Some(at) = local(at_ms, now) else {
        return String::new();
    };
    match days_ago(&at, now) {
        0 => at.format("%-I:%M %p").to_string(),
        1 => "Yesterday".to_owned(),
        2..=NAMED_DAYS => at.format("%A").to_string(),
        _ if at.year() == now.year() => at.format("%-d %b").to_string(),
        _ => at.format("%-d %b %Y").to_string(),
    }
}

fn local<Tz: TimeZone>(at_ms: i64, now: &DateTime<Tz>) -> Option<DateTime<Tz>> {
    now.timezone().timestamp_millis_opt(at_ms).single()
}

/// How many midnights passed between `at` and `now`; a time ahead of `now`,
/// from a clock running fast, is today.
fn days_ago<Tz: TimeZone>(at: &DateTime<Tz>, now: &DateTime<Tz>) -> i64 {
    (now.date_naive() - at.date_naive()).num_days().max(0)
}

fn day<Tz: TimeZone>(at_ms: i64, zone: &Tz) -> Option<chrono::NaiveDate> {
    zone.timestamp_millis_opt(at_ms)
        .single()
        .map(|at| at.date_naive())
}

#[cfg(test)]
mod tests {
    use chrono::{FixedOffset, TimeZone as _};

    use super::{long, separates, short};

    const MINUTE_MS: i64 = 60_000;

    fn zone() -> FixedOffset {
        FixedOffset::east_opt(5 * 3600 + 1800).expect("India's offset")
    }

    /// Unix milliseconds of the given local time in `zone()`.
    fn at(year: i32, month: u32, day: u32, hour: u32, minute: u32) -> i64 {
        zone()
            .with_ymd_and_hms(year, month, day, hour, minute, 0)
            .single()
            .expect("a real time")
            .timestamp_millis()
    }

    #[test]
    fn a_time_goes_before_the_first_line_and_after_an_hour_or_a_midnight() {
        let zone = zone();
        let evening = at(2026, 11, 9, 18, 12);
        assert!(separates(None, Some(evening), &zone));
        assert!(!separates(None, None, &zone), "a line with no time yet");
        assert!(!separates(
            Some(evening),
            Some(evening + 59 * MINUTE_MS),
            &zone
        ));
        assert!(separates(
            Some(evening),
            Some(evening + 60 * MINUTE_MS),
            &zone
        ));
        let late = at(2026, 11, 9, 23, 50);
        assert!(separates(Some(late), Some(late + 20 * MINUTE_MS), &zone));
    }

    #[test]
    fn a_time_names_the_day_as_near_as_it_is() {
        let now = zone()
            .with_ymd_and_hms(2026, 11, 9, 20, 0, 0)
            .single()
            .expect("now");
        let said = |at: i64| (long(at, &now), short(at, &now));
        let pair = |long: &str, short: &str| (long.to_owned(), short.to_owned());
        assert_eq!(
            said(at(2026, 11, 9, 18, 12)),
            pair("Today 6:12 PM", "6:12 PM")
        );
        assert_eq!(
            said(at(2026, 11, 8, 9, 3)),
            pair("Yesterday 9:03 AM", "Yesterday")
        );
        assert_eq!(
            said(at(2026, 11, 3, 18, 12)),
            pair("Tuesday 6:12 PM", "Tuesday")
        );
        assert_eq!(
            said(at(2026, 11, 2, 18, 12)),
            pair("2 Nov 6:12 PM", "2 Nov")
        );
        assert_eq!(
            said(at(2025, 11, 3, 18, 12)),
            pair("3 Nov 2025 6:12 PM", "3 Nov 2025")
        );
        let ahead = at(2026, 11, 9, 20, 5);
        assert_eq!(
            said(ahead),
            pair("Today 8:05 PM", "8:05 PM"),
            "a fast clock is today"
        );
    }
}
