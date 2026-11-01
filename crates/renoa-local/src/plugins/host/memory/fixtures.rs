//! Test fixtures that write a person's notebook through Mnem's own API.

use std::path::{Path, PathBuf};

use jiff::civil::DateTime;
use mnem::{dream::Trigger, memory::Memory, model::Answer};
use uuid::Uuid;

/// Where the notebook of `principal` lives under a Host data directory.
pub(crate) fn notebook(data_directory: &Path, principal: Uuid) -> PathBuf {
    data_directory
        .join("users")
        .join(principal.to_string())
        .join(super::NOTEBOOK_DIRECTORY)
}

fn at(raw: &str) -> DateTime {
    raw.parse().expect("a fixture time")
}

/// The largest `MIND.md` files Mnem accepts that grow most in a prompt: one
/// line of `&`, which escaping grows fivefold, and as many one-character
/// hunches as fit, which the view without Nudges grows by a space each.
pub(crate) fn largest_minds() -> [String; 2] {
    let max = mnem::mind::FILE_MAX;
    let escaped = {
        let frame = ["## Durable\n- ", "\n## Active\n## Hunches\n## Nudges\n"];
        let room = max - frame.concat().len();
        format!("{}{}{}", frame[0], "&".repeat(room), frame[1])
    };
    let rerendered = {
        let frame = ["## Durable\n## Active\n## Hunches\n", "## Nudges\n"];
        let hunch = "- [daily]&\n";
        let room = max - frame.concat().len();
        format!(
            "{}{}{}",
            frame[0],
            hunch.repeat(room / hunch.len()),
            frame[1]
        )
    };
    [escaped, rerendered]
}

/// Gives the notebook at `directory`, created if needed, a `MIND.md` whose
/// Active section is `active`: five notes, then a nightly Dream Mnem accepts.
/// Returns the new revision.
pub(crate) fn dream(directory: &Path, active: &str) -> String {
    dream_with_nudge(directory, active, None)
}

/// [`dream`], with one next-session nudge whose action is `nudge`. Active
/// cites the first note, so two dreams with the same `active` differ only in
/// their Nudges.
pub(crate) fn dream_with_nudge(directory: &Path, active: &str, nudge: Option<&str>) -> String {
    let memory = Memory::create(directory).expect("a notebook");
    let mut last = 0;
    for n in 0..5 {
        last = memory
            .note(
                "Fixture",
                &format!("{active}, seen {n}"),
                at("2026-10-30T09:00"),
            )
            .expect("a fixture note")
            .id;
    }
    let now = at("2026-10-31T03:00");
    let job = memory
        .dream_job(Trigger::Nightly, None, now)
        .expect("a dream job")
        .expect("five new notes make a dream due");
    let nudges = nudge.map_or_else(String::new, |action| {
        format!("- check-in | next session | - | {action} | Seen again (#{last}) | notify\n")
    });
    let draft = format!(
        "<mind>\n## Durable\n## Active\n- {active} (#0)\n## Hunches\n## Nudges\n{nudges}</mind>\n"
    );
    memory
        .apply_dream(&job.id, Answer::Reply(draft), now)
        .expect("Mnem accepts the dream");
    memory
        .mind()
        .expect("MIND.md reads")
        .expect("MIND.md is written")
        .revision
}
