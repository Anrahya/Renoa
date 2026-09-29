use super::*;

fn ms(value: &str) -> i64 {
    value
        .parse::<jiff::Timestamp>()
        .expect("timestamp")
        .as_millisecond()
}

/// The next `count` runs after `from`.
fn runs(expression: &str, timezone: &str, from: &str, count: usize) -> Vec<String> {
    let schedule = cron(expression, timezone);
    let mut now = ms(from);
    (0..count)
        .map(|_| {
            now = schedule.next_after(now).expect("next run");
            jiff::Timestamp::from_millisecond(now)
                .expect("timestamp")
                .to_string()
        })
        .collect()
}

fn refusal(expression: &str, timezone: &str) -> String {
    match cron(expression, timezone).next_after(0) {
        Err(AutomationError::Invalid(message)) => message,
        other => panic!("{expression:?} in {timezone} was not refused: {other:?}"),
    }
}

#[test]
fn cron_fields_accept_values_ranges_steps_lists_and_names() {
    // 2026-10-02 is a Friday.
    assert_eq!(
        runs("30 9 * * 1-5", "UTC", "2026-10-02T09:30:00Z", 2),
        ["2026-10-05T09:30:00Z", "2026-10-06T09:30:00Z"],
        "weekdays skip the weekend"
    );
    assert_eq!(
        runs("0 18 * * SAT,sun", "UTC", "2026-10-02T00:00:00Z", 2),
        ["2026-10-03T18:00:00Z", "2026-10-04T18:00:00Z"]
    );
    assert_eq!(
        runs("0 0 * * 7", "UTC", "2026-10-02T00:00:00Z", 1),
        runs("0 0 * * 0", "UTC", "2026-10-02T00:00:00Z", 1),
        "7 is Sunday like 0"
    );
    assert_eq!(
        runs("*/20 8-9 * * *", "UTC", "2026-10-02T07:00:00Z", 4),
        [
            "2026-10-02T08:00:00Z",
            "2026-10-02T08:20:00Z",
            "2026-10-02T08:40:00Z",
            "2026-10-02T09:00:00Z"
        ]
    );
    assert_eq!(
        runs("45/5 0 * * *", "UTC", "2026-10-02T00:00:00Z", 4),
        [
            "2026-10-02T00:45:00Z",
            "2026-10-02T00:50:00Z",
            "2026-10-02T00:55:00Z",
            "2026-10-03T00:45:00Z"
        ],
        "a stepped value runs to the end of its field"
    );
    assert_eq!(
        runs("0 10 1 jan,Jul *", "UTC", "2026-10-02T00:00:00Z", 2),
        ["2027-01-01T10:00:00Z", "2027-07-01T10:00:00Z"]
    );
}

#[test]
fn a_restricted_day_of_month_and_weekday_run_on_either() {
    // The 13th, and every Friday.
    assert_eq!(
        runs("0 12 13 * FRI", "UTC", "2026-10-02T13:00:00Z", 3),
        [
            "2026-10-09T12:00:00Z",
            "2026-10-13T12:00:00Z",
            "2026-10-16T12:00:00Z"
        ]
    );
}

#[test]
fn cron_times_are_read_in_their_timezone() {
    // An LCK match at 17:00 in Seoul is 13:30 in India.
    assert_eq!(
        runs("0 17 * * *", "Asia/Seoul", "2026-10-02T00:00:00Z", 1),
        ["2026-10-02T08:00:00Z"]
    );
    assert_eq!(
        runs("30 13 * * *", "Asia/Kolkata", "2026-10-02T00:00:00Z", 1),
        ["2026-10-02T08:00:00Z"]
    );
}

#[test]
fn clock_changes_shift_skipped_times_forward_and_run_repeated_times_once() {
    // New York skips 02:00-03:00 on 8 March 2026.
    assert_eq!(
        runs("30 2 * * *", "America/New_York", "2026-03-07T12:00:00Z", 2),
        ["2026-03-08T07:30:00Z", "2026-03-09T06:30:00Z"],
        "02:30 does not exist and runs at 03:30"
    );
    assert_eq!(
        runs(
            "0,30 2,3 * * *",
            "America/New_York",
            "2026-03-08T06:00:00Z",
            3
        ),
        [
            "2026-03-08T07:00:00Z",
            "2026-03-08T07:30:00Z",
            "2026-03-09T06:00:00Z"
        ],
        "a skipped 02:00 and the real 03:00 are one run"
    );
    // Lord Howe Island skips 02:00-02:30 on 4 October 2026, so a skipped
    // 02:15 runs at 02:45, after the real 02:40.
    assert_eq!(
        runs(
            "0,15,40 2 * * *",
            "Australia/Lord_Howe",
            "2026-10-03T14:00:00Z",
            3
        ),
        [
            "2026-10-03T15:30:00Z",
            "2026-10-03T15:40:00Z",
            "2026-10-03T15:45:00Z"
        ]
    );
    // New York repeats 01:00-02:00 on 1 November 2026.
    assert_eq!(
        runs("30 1 * * *", "America/New_York", "2026-10-31T12:00:00Z", 2),
        ["2026-11-01T05:30:00Z", "2026-11-02T06:30:00Z"],
        "a repeated 01:30 runs once"
    );
}

#[test]
fn cron_runs_at_least_five_minutes_apart() {
    for expression in ["*/5 * * * *", "0,5,55 * * * *", "0 * * * *"] {
        cron(expression, "UTC")
            .next_after(0)
            .unwrap_or_else(|error| panic!("{expression} refused: {error}"));
    }
    for (expression, minute) in [
        ("* * * * *", "*"),
        ("*/4 * * * *", "*/4"),
        ("0,1 9 * * *", "0,1"),
        ("0,57 * * * *", "0,57"),
    ] {
        assert_eq!(
            refusal(expression, "UTC"),
            format!(
                "cron minute field \"{minute}\" runs less than 5 minutes apart; leave at least 5 minutes between runs, e.g. \"*/5\" or \"0,30\""
            )
        );
    }
}

#[test]
fn invalid_cron_calls_are_told_how_to_fix_them() {
    assert_eq!(
        refusal("0 9 * * * *", "UTC"),
        "cron expression needs 5 fields \"minute hour day-of-month month weekday\", e.g. \"30 9 * * 1-5\" for 09:30 on weekdays; \"0 9 * * * *\" has 6"
    );
    assert_eq!(
        refusal("0 24 * * *", "UTC"),
        "cron hour value \"24\" must be 0-23"
    );
    assert_eq!(
        refusal("0 9 * * MONDAY", "UTC"),
        "cron weekday value \"MONDAY\" must be 0-7 or SUN-SAT, where 0 and 7 are Sunday"
    );
    assert_eq!(
        refusal("0 9 * 13 *", "UTC"),
        "cron month value \"13\" must be 1-12 or JAN-DEC"
    );
    assert_eq!(
        refusal("0 9 20-10 * *", "UTC"),
        "cron day-of-month range \"20-10\" runs backwards; write the smaller value first"
    );
    assert_eq!(
        refusal("*/0 9 * * *", "UTC"),
        "cron minute step \"0\" must be a whole number of at least 1"
    );
    assert_eq!(
        refusal("0 9 , * *", "UTC"),
        "cron day-of-month value \"\" must be 1-31"
    );
    assert_eq!(
        refusal("0 0 31 2 *", "UTC"),
        "cron expression never runs; check that its day-of-month exists in its month"
    );
    assert_eq!(
        refusal("0 9 * * *", "IST"),
        "timezone \"IST\" is not an IANA timezone name; use one like \"Asia/Kolkata\" or \"America/New_York\""
    );
    assert_eq!(
        refusal(&"0 ".repeat(65), "UTC"),
        "cron expression is too long; use at most 128 characters"
    );
    assert_eq!(
        runs("0 0 29 2 *", "UTC", "2026-10-02T00:00:00Z", 1),
        ["2028-02-29T00:00:00Z"],
        "a leap day waits for its year"
    );
}
