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

/// A `MIND.md` of Mnem's largest size whose one Durable line repeats `text`.
fn one_line_mind(text: &str) -> String {
    let frame = ["## Durable\n- ", "\n## Active\n## Hunches\n## Nudges\n"];
    let room = mnem::mind::FILE_MAX - frame.concat().len();
    let line: String = text.chars().cycle().take(room).collect();
    format!("{}{line}{}", frame[0], frame[1])
}

/// The largest `MIND.md` files of ordinary prose Mnem accepts: one long line,
/// and as many one-letter hunches as fit, which the view without Nudges grows
/// by a space each.
pub(crate) fn largest_prose_minds() -> [String; 2] {
    let frame = ["## Durable\n## Active\n## Hunches\n", "## Nudges\n"];
    let hunch = "- [daily]x\n";
    let room = mnem::mind::FILE_MAX - frame.concat().len();
    let hunches = format!(
        "{}{}{}",
        frame[0],
        hunch.repeat(room / hunch.len()),
        frame[1]
    );
    [
        one_line_mind("Walks the dog at dawn; reads before bed, (mostly) fiction! "),
        hunches,
    ]
}

/// The largest `MIND.md` full of `&`, which escaping grows fivefold.
pub(crate) fn escaping_mind() -> String {
    one_line_mind("&")
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
