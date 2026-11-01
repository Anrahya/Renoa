//! Test fixtures that write a person's notebook through Mnem's own API.

use std::path::{Path, PathBuf};

use jiff::civil::DateTime;
use mnem::{
    dream::Trigger,
    memory::{Memory, MemoryError},
    model::Answer,
};
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

/// Runs a Dream step again while Mnem reports another dream. Dream takes its
/// lock with a non-blocking `flock`, and a child another test forks inherits
/// the lock's open file until it execs, so a lock this fixture just released
/// can still look held for a moment.
fn unbusy<T>(mut step: impl FnMut() -> Result<T, MemoryError>) -> Result<T, MemoryError> {
    for _ in 0..200 {
        match step() {
            Err(MemoryError::Conflict { .. }) => {
                std::thread::sleep(std::time::Duration::from_millis(5));
            }
            settled => return settled,
        }
    }
    step()
}

/// Gives the notebook at `directory`, created if needed, a `MIND.md` whose
/// Active section is `active`: five notes, then a nightly Dream Mnem accepts.
/// Returns the new revision.
pub(crate) fn dream(directory: &Path, active: &str) -> String {
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
    let job = unbusy(|| memory.dream_job(Trigger::Nightly, None, now))
        .expect("a dream job")
        .expect("five new notes make a dream due");
    let draft = format!(
        "<mind>\n## Durable\n## Active\n- {active} (#{last})\n## Hunches\n## Nudges\n</mind>\n"
    );
    unbusy(|| memory.apply_dream(&job.id, Answer::Reply(draft.clone()), now))
        .expect("Mnem accepts the dream");
    memory
        .mind()
        .expect("MIND.md reads")
        .expect("MIND.md is written")
        .revision
}
