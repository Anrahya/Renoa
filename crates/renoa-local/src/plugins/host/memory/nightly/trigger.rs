//! When the nightly run is due and for whom: the trigger `renoa.memory`
//! contributes, which the scheduler keeps converged.

use jiff::{Timestamp, tz::TimeZone};
use mnem::pass::DAY_START_HOUR;
use renoa_kernel::AgentId;
use rusqlite::Connection;

use super::super::notebook_zone;
use crate::{
    AutomationSchedule, AutomationSpec,
    host::definition::main_agent,
    plugins::{
        PluginError,
        host::{HostPluginId, state},
    },
};

const NAME: &str = "renoa.memory nightly";
const TASK: &str = "Compact every notebook on this Host, then dream: write each person's MIND.md from what was noted since the last dream.";

/// The agent whose memory session drives every notebook: the Host's main
/// agent, while `renoa.memory` is on for it. One driver per notebook, so the
/// `mnem` command's compactor must never run on one too.
pub(crate) fn driver(db: &Connection) -> Result<Option<AgentId>, PluginError> {
    let Some(agent) = main_agent(db)? else {
        return Ok(None);
    };
    Ok(state::enabled_in(db, agent, HostPluginId::Memory)?.then_some(agent))
}

/// The nightly trigger, for the [`driver`], at Mnem's [`DAY_START_HOUR`] in
/// the notebook zone as of `now_ms`: the hour one pass day ends, while the
/// person is asleep.
pub(crate) fn wanted(db: &Connection, now_ms: i64) -> Result<Option<AutomationSpec>, PluginError> {
    let Some(agent) = driver(db)? else {
        return Ok(None);
    };
    let at = Timestamp::from_millisecond(now_ms)
        .map_err(|error| PluginError::Invalid(format!("the scheduler's time: {error}")))?;
    Ok(Some(AutomationSpec {
        agent_id: agent,
        name: NAME.to_owned(),
        prompt: TASK.to_owned(),
        schedule: schedule(&notebook_zone(db)?, at),
        enabled: true,
    }))
}

/// [`DAY_START_HOUR`] every day in `zone`. A cron schedule names an IANA zone, so a zone
/// without one, a fixed offset, a `TZ` rule, or a zone file with no name
/// such as a copied `/etc/localtime`, is scheduled in UTC at the minute its
/// offset at `at` reads that hour: the minute Dream's `Now:` shows, which reads
/// the same zone. The scheduler converges the trigger before each due query,
/// so after a change of offset the next query moves it, and the night's run
/// comes at that hour in the new offset. A Host down across both the change and
/// the old fire time skips that night, since the moved schedule is due again
/// from when it comes back; an IANA zone's missed night catches up instead.
fn schedule(zone: &TimeZone, at: Timestamp) -> AutomationSchedule {
    if let Some(name) = zone.iana_name() {
        return AutomationSchedule::Cron {
            expression: format!("0 {DAY_START_HOUR} * * *"),
            timezone: name.to_owned(),
        };
    }
    let utc = (i32::from(DAY_START_HOUR) * 3600 - zone.to_offset(at).seconds())
        .div_euclid(60)
        .rem_euclid(24 * 60);
    AutomationSchedule::Cron {
        expression: format!("{} {} * * *", utc % 60, utc / 60),
        timezone: "UTC".to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use jiff::{
        Timestamp,
        civil::time,
        tz::{Offset, TimeZone},
    };

    use mnem::pass::DAY_START_HOUR;

    use super::schedule;
    use crate::{AutomationSchedule, plugins::host::memory::local_time};

    /// The trigger's expression as of `at`, and the wall time in `zone` at the
    /// minute it names on `at`'s day, in the schedule's own zone.
    fn next_in(zone: &TimeZone, at: &str) -> (String, jiff::civil::Time) {
        let at: Timestamp = at.parse().expect("time");
        let AutomationSchedule::Cron {
            expression,
            timezone,
        } = schedule(zone, at)
        else {
            panic!("a cron schedule");
        };
        let fields: Vec<i8> = expression
            .split(' ')
            .take(2)
            .map(|field| field.parse().expect("a fixed minute and hour"))
            .collect();
        let named = TimeZone::get(&timezone).expect("an IANA zone");
        let fires = at
            .to_zoned(named.clone())
            .date()
            .at(fields[1], fields[0], 0, 0)
            .to_zoned(named)
            .expect("the minute")
            .timestamp();
        (expression, local_time(zone, fires).time())
    }

    #[test]
    fn a_zone_without_a_name_runs_at_the_day_start_in_its_own_offset() {
        let start = time(DAY_START_HOUR, 0, 0, 0);
        for (zone, at, expression) in [
            (
                TimeZone::fixed(Offset::from_seconds(19_800).expect("offset")),
                "2026-11-02T12:00:00Z",
                "30 5 * * *",
            ),
            (
                TimeZone::fixed(Offset::from_seconds(-10_800).expect("offset")),
                "2026-11-02T12:00:00Z",
                "0 14 * * *",
            ),
            (
                TimeZone::posix("EST5EDT,M3.2.0,M11.1.0").expect("rule"),
                "2026-07-01T12:00:00Z",
                "0 15 * * *",
            ),
            (
                TimeZone::posix("EST5EDT,M3.2.0,M11.1.0").expect("rule"),
                "2026-12-01T12:00:00Z",
                "0 16 * * *",
            ),
            (
                TimeZone::get("Asia/Kolkata").expect("zone"),
                "2026-11-02T12:00:00Z",
                "0 11 * * *",
            ),
        ] {
            assert_eq!(
                next_in(&zone, at),
                (expression.to_owned(), start),
                "{zone:?}"
            );
        }
    }

    /// Each night's run, as the trigger converged half an hour before the day
    /// start on that night's date, across a spring-forward and a fall-back of
    /// a `TZ` rule: once a night, at the day start local, on consecutive dates.
    #[test]
    fn a_rule_runs_at_the_day_start_on_each_side_of_its_changes() {
        let zone = TimeZone::posix("EST5EDT,M3.2.0,M11.1.0").expect("rule");
        let start = time(DAY_START_HOUR, 0, 0, 0);
        for (before, after) in [
            ("2026-03-07T15:30:00Z", "2026-03-08T14:30:00Z"),
            ("2026-10-31T14:30:00Z", "2026-11-01T15:30:00Z"),
        ] {
            let nights: Vec<_> = [before, after]
                .into_iter()
                .map(|at| {
                    let (_, wall) = next_in(&zone, at);
                    let date = local_time(&zone, at.parse().expect("time")).date();
                    (date, wall)
                })
                .collect();
            assert_eq!(nights[0].1, start, "{before}");
            assert_eq!(nights[1].1, start, "{after}");
            assert_eq!(nights[0].0.tomorrow().expect("date"), nights[1].0);
        }
    }
}
