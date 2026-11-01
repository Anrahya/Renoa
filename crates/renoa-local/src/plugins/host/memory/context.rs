//! What `renoa.memory` adds to the two context points, and the zone the
//! notebook keeps time in.
//!
//! The notebook is the person's, so its wall time is theirs: the Host's main
//! agent's `renoa.time` zone, or the Host zone while there is no main agent.
//! Every agent writes `[at …]` and `[on …]` in that zone, and one whose own
//! clock shows another zone, or none, is told the notebook's with each message.

use std::path::Path;

use jiff::{Timestamp, fmt::strtime, tz::TimeZone};
use renoa_agent_loop::ContextContribution;
use renoa_kernel::AgentId;
use uuid::Uuid;

use super::{PLUGIN_ID, mind};
use crate::{
    host::definition::main_agent,
    plugins::{
        PluginError,
        host::{HostPluginId, session_context::Contribution, state, time::TimeSettings},
    },
};

/// The line the `MIND.md` block opens with.
const MIND_HEADING: &str = "MIND.md: what the shared notebook of the person you are talking to adds up to. Use memory_recall for the notes behind it.";
/// What the main agent, the only one shown the Nudges, is told about them.
const NUDGES_HEADING: &str =
    "Don't act on a nudge before it is handed to you, and one that was handed over is done.";

/// The largest block this plugin adds, as the prompt carries it: the headings,
/// then a view of `MIND.md` escaped. Mnem refuses a file over
/// `mind::FILE_MAX`; the half again on top is room for the view without
/// Nudges, which re-renders the file, and for ordinary escaping. A file that
/// escapes past it, such as one full of `&`, is left out and reported: the
/// block goes out with every turn, so it stays small.
pub(crate) const SESSION_BLOCK_MAX: usize = MIND_HEADING.len()
    + 1
    + NUDGES_HEADING.len()
    + 1
    + mnem::mind::FILE_MAX
    + mnem::mind::FILE_MAX / 2;

/// The zone the notebook keeps wall time in: the Host's main agent's
/// `renoa.time` zone, or the Host zone while there is no main agent.
pub(crate) fn notebook_zone(db: &rusqlite::Connection) -> Result<TimeZone, PluginError> {
    TimeSettings::zone_of(db, main_agent(db)?)
}

/// The `message_context` entry naming the notebook's zone and the time there,
/// for a message to `agent` from a person admitted at `observed_at_ms`. It is
/// left out when `renoa.time` already shows `agent` that zone, since then the
/// agent's own clock is the notebook's.
pub(crate) fn message_entry(
    db: &rusqlite::Connection,
    agent: AgentId,
    observed_at_ms: i64,
) -> Result<Option<ContextContribution>, PluginError> {
    if !state::enabled_in(db, agent, HostPluginId::Memory)? {
        return Ok(None);
    }
    let invalid = |error: jiff::Error| PluginError::Invalid(error.to_string());
    let now = Timestamp::from_millisecond(observed_at_ms).map_err(invalid)?;
    let zone = notebook_zone(db)?;
    if state::enabled_in(db, agent, HostPluginId::Time)?
        && same_wall_time(&TimeSettings::zone_of(db, Some(agent))?, &zone, now)
    {
        return Ok(None);
    }
    let there = now.to_zoned(zone);
    let text = format!(
        "Memory times are in {}; it is now {} there.",
        strtime::format("%Q", &there).map_err(invalid)?,
        strtime::format("%Y-%m-%d %H:%M", &there).map_err(invalid)?
    );
    ContextContribution::plugin(PLUGIN_ID, text)
        .map(Some)
        .map_err(|error| PluginError::Invalid(error.to_string()))
}

/// Whether two zones show the same wall time at `at`: the same IANA zone, or,
/// for a zone without a name, the same offset.
fn same_wall_time(left: &TimeZone, right: &TimeZone, at: Timestamp) -> bool {
    match (left.iana_name(), right.iana_name()) {
        (Some(left), Some(right)) => left == right,
        _ => left.to_offset(at) == right.to_offset(at),
    }
}

/// The `session_context` block for `agent` talking to `principal`: their
/// `MIND.md`, whole for the Host's main agent, and without its Nudges for
/// every other agent, since only the main agent acts on them. Each view
/// carries its own Mnem revision, so a nudge-only change leaves every other
/// agent's prompt as it was.
pub(crate) fn session_block(
    db: &rusqlite::Connection,
    data_directory: &Path,
    agent: AgentId,
    principal: Uuid,
) -> Result<Option<Contribution>, String> {
    let Some(mind) = mind(data_directory, principal)? else {
        return Ok(None);
    };
    if main_agent(db).map_err(|error| error.to_string())? == Some(agent) {
        return Ok(Some(Contribution {
            revision: mind.revision,
            text: format!("{MIND_HEADING} {NUDGES_HEADING}\n{}", mind.text),
        }));
    }
    let shared = mind.without_nudges();
    Ok(Some(Contribution {
        revision: shared.revision,
        text: format!("{MIND_HEADING}\n{}", shared.text),
    }))
}

#[cfg(test)]
mod tests {
    use jiff::{Timestamp, tz::TimeZone};

    use super::same_wall_time;

    #[test]
    fn a_zone_without_a_name_shows_the_same_wall_time_by_offset() {
        let zone = |name: &str| TimeZone::get(name).expect("zone");
        let fixed = |minutes: i32| {
            let offset = jiff::tz::Offset::from_seconds(minutes * 60).expect("offset");
            let zone = TimeZone::fixed(offset);
            assert_eq!(zone.iana_name(), None, "{offset} is unnamed");
            zone
        };
        let winter: Timestamp = "2026-01-15T12:00:00Z".parse().expect("winter");
        let summer: Timestamp = "2026-07-15T12:00:00Z".parse().expect("summer");
        for (left, right, at, same) in [
            (zone("Asia/Kolkata"), fixed(330), winter, true),
            (fixed(330), zone("Asia/Kolkata"), winter, true),
            (fixed(330), fixed(330), winter, true),
            (fixed(330), fixed(345), winter, false),
            (zone("Asia/Kathmandu"), fixed(330), winter, false),
            (zone("America/New_York"), fixed(-300), winter, true),
            (zone("America/New_York"), fixed(-300), summer, false),
            // Named zones agree by name, not by today's offset.
            (
                zone("America/New_York"),
                zone("America/Toronto"),
                winter,
                false,
            ),
            (zone("Asia/Kolkata"), zone("Asia/Kolkata"), winter, true),
        ] {
            assert_eq!(
                same_wall_time(&left, &right, at),
                same,
                "{:?} and {:?} at {at}",
                left.iana_name(),
                right.iana_name()
            );
        }
    }
}
