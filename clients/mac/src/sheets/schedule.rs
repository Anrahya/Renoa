//! A Host schedule in plain words, as the phone writes it
//! (`agents/ScheduleWords.kt` in the Android app): "Every day at 6:30 PM",
//! "Every weekday at 7:30 AM", "Every Thursday at 7:15 PM".

use chrono::{DateTime, NaiveTime, TimeZone};

use crate::stamp::clock;
use serde::Deserialize;

/// What a schedule reads when its shape is not one [`words`] knows.
pub const ON_A_SCHEDULE: &str = "On a schedule";

const DAYS: [&str; 7] = [
    "Sunday",
    "Monday",
    "Tuesday",
    "Wednesday",
    "Thursday",
    "Friday",
    "Saturday",
];

/// The Host's three-letter weekday names, which it reads in any case, Sunday
/// first as 0.
const DAY_NAMES: [&str; 7] = ["SUN", "MON", "TUE", "WED", "THU", "FRI", "SAT"];

/// Monday first, as a week is read.
const WEEK_ORDER: [usize; 7] = [1, 2, 3, 4, 5, 6, 0];

/// A Host schedule: `once` carries `at`; `cron` carries `expression`, read
/// in `timezone`.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct Schedule {
    pub kind: String,
    pub at: Option<String>,
    pub expression: Option<String>,
    pub timezone: Option<String>,
}

/// `schedule` in plain words. A cron schedule's times are read in its own
/// timezone, which is named when it is not `here_zone`, this Mac's; a
/// one-off time is shown in `here`. A shape this does not know reads
/// [`ON_A_SCHEDULE`] rather than a guess.
pub fn words<Tz: TimeZone>(schedule: &Schedule, here: &Tz, here_zone: Option<&str>) -> String
where
    Tz::Offset: std::fmt::Display,
{
    let said = match schedule.kind.as_str() {
        "once" => schedule.at.as_deref().and_then(|at| once(at, here)),
        "cron" => schedule
            .expression
            .as_deref()
            .and_then(cron)
            .and_then(|words| {
                let zone = schedule.timezone.as_deref()?;
                Some(if Some(zone) == here_zone {
                    words
                } else {
                    let city = zone.rsplit('/').next().unwrap_or(zone).replace('_', " ");
                    format!("{words}, {city} time")
                })
            }),
        _ => None,
    };
    said.unwrap_or_else(|| ON_A_SCHEDULE.to_owned())
}

fn once<Tz: TimeZone>(at: &str, here: &Tz) -> Option<String>
where
    Tz::Offset: std::fmt::Display,
{
    let time = DateTime::parse_from_rfc3339(at).ok()?.with_timezone(here);
    Some(format!(
        "Once, on {} at {}",
        time.format("%a %-d %b"),
        clock(time.time())
    ))
}

/// The five fields "minute hour day-of-month month weekday", for the shapes
/// people set most.
fn cron(expression: &str) -> Option<String> {
    let fields: Vec<&str> = expression.split_whitespace().collect();
    let [minute, hour, day, month, weekday] = fields[..] else {
        return None;
    };
    let m: u32 = minute.parse().ok().filter(|m| *m <= 59)?;
    if month != "*" {
        return None;
    }
    let Some(h) = hour.parse::<u32>().ok().filter(|h| *h <= 23) else {
        if day != "*" || weekday != "*" {
            return None;
        }
        let past = |every: &str| {
            if m == 0 {
                format!("{every}, on the hour")
            } else {
                format!("{every} at {m} minutes past")
            }
        };
        if hour == "*" {
            return Some(past("Every hour"));
        }
        // The Host counts a step from midnight each day, so only a step that
        // divides the day keeps its gap.
        let step: u32 = hour.strip_prefix("*/")?.parse().ok()?;
        return ((2..=12).contains(&step) && 24 % step == 0)
            .then(|| past(&format!("Every {step} hours")));
    };
    let at = format!("at {}", clock(NaiveTime::from_hms_opt(h, m, 0)?));
    match (day, weekday) {
        ("*", "*") => Some(format!("Every day {at}")),
        ("*", weekday) => days(weekday).map(|days| format!("{days} {at}")),
        // A day some months lack is skipped in those months, so it is not
        // "every month".
        (day, "*") => {
            let day: u32 = day.parse().ok().filter(|day| (1..=28).contains(day))?;
            Some(format!("On the {} of every month {at}", ordinal(day)))
        }
        _ => None,
    }
}

/// A weekday field as words, "Every weekday" or "Every Monday and Friday",
/// or `None` for steps and other shapes.
fn days(field: &str) -> Option<String> {
    let mut chosen = [false; 7];
    for part in field.split(',') {
        let bounds = part.split('-').map(weekday).collect::<Option<Vec<_>>>()?;
        match bounds[..] {
            [one] => chosen[one % 7] = true,
            [from, to] if from <= to => (from..=to).for_each(|day| chosen[day % 7] = true),
            _ => return None,
        }
    }
    if chosen.iter().all(|day| *day) {
        return Some("Every day".to_owned());
    }
    if chosen == [false, true, true, true, true, true, false] {
        return Some("Every weekday".to_owned());
    }
    let named: Vec<&str> = WEEK_ORDER
        .iter()
        .filter(|day| chosen[**day])
        .map(|day| DAYS[*day])
        .collect();
    Some(format!("Every {}", listed(&named)))
}

/// A weekday as the Host reads it: 0 to 7, where 7 is Sunday like 0, or a
/// three-letter name.
fn weekday(text: &str) -> Option<usize> {
    text.parse::<usize>()
        .ok()
        .filter(|day| *day <= 7)
        .or_else(|| {
            DAY_NAMES
                .iter()
                .position(|name| name.eq_ignore_ascii_case(text))
        })
}

/// "A", "A and B", or "A, B and C".
pub fn listed(words: &[&str]) -> String {
    match words {
        [] => String::new(),
        [one] => (*one).to_owned(),
        [rest @ .., last] => format!("{} and {last}", rest.join(", ")),
    }
}

fn ordinal(n: u32) -> String {
    let suffix = match (n % 100, n % 10) {
        (11..=13, _) => "th",
        (_, 1) => "st",
        (_, 2) => "nd",
        (_, 3) => "rd",
        _ => "th",
    };
    format!("{n}{suffix}")
}

#[cfg(test)]
mod tests {
    use chrono::FixedOffset;

    use super::{ON_A_SCHEDULE, Schedule, listed, words};

    const KOLKATA: &str = "Asia/Kolkata";

    fn india() -> FixedOffset {
        FixedOffset::east_opt(5 * 3600 + 1800).expect("India's offset")
    }

    fn schedule(
        kind: &str,
        at: Option<&str>,
        expression: Option<&str>,
        timezone: Option<&str>,
    ) -> Schedule {
        Schedule {
            kind: kind.to_owned(),
            at: at.map(str::to_owned),
            expression: expression.map(str::to_owned),
            timezone: timezone.map(str::to_owned),
        }
    }

    fn cron(expression: &str) -> String {
        words(
            &schedule("cron", None, Some(expression), Some(KOLKATA)),
            &india(),
            Some(KOLKATA),
        )
    }

    /// The phone's cases (`ScheduleWordsTest.kt`), so both apps say the same.
    #[test]
    fn the_schedules_people_set_read_as_plain_words() {
        for (expression, said) in [
            ("30 18 * * *", "Every day at 6:30 PM"),
            ("0 13 * * *", "Every day at 1 PM"),
            ("0 0 * * *", "Every day at 12 AM"),
            ("30 7 * * 1-5", "Every weekday at 7:30 AM"),
            ("30 7 * * mon-FRI", "Every weekday at 7:30 AM"),
            ("15 19 * * 4", "Every Thursday at 7:15 PM"),
            ("0 10 * * 0", "Every Sunday at 10 AM"),
            ("0 10 * * 7", "Every Sunday at 10 AM"),
            ("0 10 * * 0,6", "Every Saturday and Sunday at 10 AM"),
            (
                "0 8 * * 1,3,5",
                "Every Monday, Wednesday and Friday at 8 AM",
            ),
            ("0 8 * * 0-6", "Every day at 8 AM"),
            ("0 9 1 * *", "On the 1st of every month at 9 AM"),
            ("0 9 22 * *", "On the 22nd of every month at 9 AM"),
            ("0 9 13 * *", "On the 13th of every month at 9 AM"),
            ("0 9 28 * *", "On the 28th of every month at 9 AM"),
            ("0 * * * *", "Every hour, on the hour"),
            ("20 * * * *", "Every hour at 20 minutes past"),
            ("0 */3 * * *", "Every 3 hours, on the hour"),
            ("45 */2 * * *", "Every 2 hours at 45 minutes past"),
            ("0 */12 * * *", "Every 12 hours, on the hour"),
        ] {
            assert_eq!(cron(expression), said, "{expression}");
        }
    }

    #[test]
    fn a_shape_it_does_not_know_is_not_guessed() {
        for expression in [
            "*/10 * * * *",
            "0 9 * 1 *",
            "0 9 1 * 1",
            "0 9 * * 1/2",
            "0 9 * * 5-1",
            "0 9 * * 8",
            "0 9-17 * * *",
            "0 9 * *",
            "0 24 * * *",
            // A step from midnight that does not divide the day.
            "0 */1 * * *",
            "0 */5 * * *",
            "0 */7 * * *",
            "0 */16 * * *",
            // Months without that day skip it.
            "0 9 29 * *",
            "0 9 31 * *",
        ] {
            assert_eq!(cron(expression), ON_A_SCHEDULE, "{expression}");
        }
        assert_eq!(
            words(
                &schedule("weekly", None, None, None),
                &india(),
                Some(KOLKATA)
            ),
            ON_A_SCHEDULE
        );
        assert_eq!(
            words(
                &schedule("once", Some("tomorrow"), None, None),
                &india(),
                Some(KOLKATA)
            ),
            ON_A_SCHEDULE
        );
    }

    #[test]
    fn a_timezone_other_than_this_macs_is_named() {
        let at = |zone: Option<&str>| {
            words(
                &schedule("cron", None, Some("0 9 * * *"), zone),
                &india(),
                Some(KOLKATA),
            )
        };
        assert_eq!(
            at(Some("America/New_York")),
            "Every day at 9 AM, New York time"
        );
        assert_eq!(at(Some(KOLKATA)), "Every day at 9 AM");
        assert_eq!(
            at(None),
            ON_A_SCHEDULE,
            "a schedule without its timezone is not shown on this clock"
        );
    }

    #[test]
    fn a_one_off_time_is_shown_on_this_macs_clock() {
        assert_eq!(
            words(
                &schedule("once", Some("2026-10-30T13:30:00Z"), None, None),
                &india(),
                Some(KOLKATA)
            ),
            "Once, on Fri 30 Oct at 7 PM"
        );
    }

    #[test]
    fn a_list_reads_as_people_write_it() {
        assert_eq!(listed(&["Chef"]), "Chef");
        assert_eq!(listed(&["Chef", "Scout"]), "Chef and Scout");
        assert_eq!(listed(&["Chef", "Scout", "Deals"]), "Chef, Scout and Deals");
    }
}
