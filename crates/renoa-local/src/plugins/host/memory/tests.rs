use std::{fs, path::Path};

use jiff::{Timestamp, civil::DateTime, tz::TimeZone};
use mnem::memory::{AUTHOR_MAX, Memory, strip_author};
use mnem::span::Span;
use renoa_agent::ToolErrorCode;
use renoa_kernel::AgentId;
use uuid::Uuid;

use super::{author, first_note, fixtures, local_time, mind, note, recall, zoom};

fn at(raw: &str) -> DateTime {
    raw.parse().expect("a test time")
}

/// Mnem reads `author` back from a note's prefix exactly when it accepts it.
fn accepted(author: &str) -> bool {
    strip_author(&format!("[by {author}] x")).0 == Some(author)
}

#[test]
fn an_author_is_derived_from_the_agent_name_within_mnems_rules() {
    let agent = AgentId::new();
    let long = "Navigator of the Seven Seas and Beyond";
    // Byte 32 falls inside the ninth three-byte check mark.
    let marks = "Kcal! ✓✓✓✓✓✓✓✓✓✓";
    for (name, expected) in [
        ("Arcee", "Arcee".to_owned()),
        ("[Scout] the Second", "Scout the Second".to_owned()),
        ("  Kcal\n\r\tcounter ", "Kcalcounter".to_owned()),
        (long, "Navigator of the Seven Seas and".to_owned()),
    ] {
        let derived = author(name, agent);
        assert_eq!(derived, expected, "{name:?}");
        assert!(accepted(&derived), "Mnem accepts {derived:?}");
    }

    let cut = author(marks, agent);
    assert_eq!(cut, "Kcal! ✓✓✓✓✓✓✓✓", "cut after the last whole character");
    assert!(
        cut.len() <= AUTHOR_MAX && accepted(&cut),
        "Mnem accepts {cut:?}"
    );
    assert!(accepted(&author("Ünïcödé Ågent", agent)));

    let id = agent.to_string();
    for empty in ["[]", " [ ] ", "\n\t", "[\u{7}]"] {
        let fallback = author(empty, agent);
        assert_eq!(fallback, format!("agent-{}", &id[..8]), "{empty:?}");
        assert!(accepted(&fallback), "Mnem accepts {fallback:?}");
    }
}

#[test]
fn a_note_is_dated_in_the_zone_it_is_given() {
    // 2026-08-31T20:30:00Z: still the 31st in UTC, already the 1st in Kolkata.
    let instant = Timestamp::from_second(1_788_208_200).expect("instant");
    let kolkata = TimeZone::get("Asia/Kolkata").expect("zone");
    assert_eq!(local_time(&kolkata, instant), at("2026-09-01T02:00"));
    assert_eq!(local_time(&TimeZone::UTC, instant), at("2026-08-31T20:30"));
}

fn users(data: &Path) -> std::path::PathBuf {
    data.join("users")
}

#[test]
fn reads_without_a_notebook_are_empty_and_create_nothing() {
    let directory = tempfile::tempdir().expect("data directory");
    let data = directory.path();
    let person = Uuid::new_v4();
    assert!(recall(data, person, ".", 20).expect("recall").is_empty());
    let block = Span::parse_block("0-1").expect("block");
    assert!(zoom(data, person, block).expect("zoom").is_empty());
    assert_eq!(mind(data, person).expect("mind"), None);
    assert!(!users(data).exists(), "nothing is created for a read");
}

#[test]
fn a_refused_first_note_leaves_nothing_and_a_kept_one_creates_the_notebook() {
    let directory = tempfile::tempdir().expect("data directory");
    let data = directory.path();
    let person = Uuid::new_v4();
    let now = at("2026-10-31T09:00");
    for refused in [
        "",
        "  \t",
        "two\nlines",
        "[updates #0] corrects nothing yet",
        &"x".repeat(281),
    ] {
        let error = note(data, person, "Arcee", refused, now).expect_err("Mnem refuses it");
        assert_eq!(error.code(), ToolErrorCode::InvalidInput, "{refused:?}");
        assert!(!users(data).exists(), "{refused:?} left a directory");
    }

    let kept = note(
        data,
        person,
        "Arcee",
        "Dentist on Friday [on 2026-11-06]",
        now,
    )
    .expect("kept");
    assert_eq!(kept.id, 0);
    let person_directory = users(data).join(person.to_string());
    let entries: Vec<String> = fs::read_dir(&person_directory)
        .expect("person directory")
        .map(|entry| {
            entry
                .expect("entry")
                .file_name()
                .to_string_lossy()
                .into_owned()
        })
        .collect();
    assert_eq!(entries, ["mnem"], "no staging is left behind");
    let notes = recall(data, person, "dentist", 20).expect("recall");
    let [found] = notes.as_slice() else {
        panic!("one note: {notes:?}");
    };
    assert_eq!(found.author.as_deref(), Some("Arcee"));
    assert_eq!(found.happened.as_deref(), Some("2026-11-06"));
    assert_eq!(found.written, "2026-10-31");

    let refused = note(data, person, "Arcee", "[updates #7] no such note", now);
    assert!(
        refused.is_err(),
        "a later note is checked against the notebook"
    );
    assert_eq!(recall(data, person, ".", 20).expect("recall").len(), 1);
    assert!(
        recall(data, Uuid::new_v4(), ".", 20)
            .expect("recall")
            .is_empty(),
        "another person's notebook is their own"
    );
}

#[test]
fn a_first_note_that_loses_the_race_joins_the_winners_notebook() {
    let directory = tempfile::tempdir().expect("data directory");
    let data = directory.path();
    let person = Uuid::new_v4();
    let now = at("2026-10-31T09:00");
    note(data, person, "Arcee", "Lives near the lake", now).expect("the winner's note");
    // The loser looked before the winner renamed its notebook into place.
    let joined = first_note(data, person, "Scout", "Walks every evening", now).expect("joined");
    assert_eq!(joined.id, 1, "written into the winner's notebook");
    let person_directory = users(data).join(person.to_string());
    assert_eq!(
        fs::read_dir(&person_directory)
            .expect("person directory")
            .count(),
        1,
        "the loser's staging is gone"
    );
    assert_eq!(recall(data, person, ".", 20).expect("recall").len(), 2);
}

#[cfg(unix)]
#[test]
fn a_linked_notebook_is_refused_not_followed() {
    let directory = tempfile::tempdir().expect("data directory");
    let data = directory.path();
    let person = Uuid::new_v4();
    let elsewhere = data.join("elsewhere");
    Memory::create(&elsewhere).expect("a notebook elsewhere");
    let person_directory = users(data).join(person.to_string());
    fs::create_dir_all(&person_directory).expect("person directory");
    std::os::unix::fs::symlink(&elsewhere, person_directory.join("mnem")).expect("link");
    let error = recall(data, person, ".", 20).expect_err("refused");
    assert_eq!(error.code(), ToolErrorCode::InvalidInput);
    assert!(note(data, person, "Arcee", "x", at("2026-10-31T09:00")).is_err());
    assert!(
        Memory::open(&elsewhere)
            .expect("open")
            .recall(".", 20)
            .expect("recall")
            .is_empty(),
        "nothing was written through the link"
    );
}

#[test]
fn mind_reads_what_dream_wrote() {
    let directory = tempfile::tempdir().expect("data directory");
    let data = directory.path();
    let person = Uuid::new_v4();
    let revision = fixtures::dream(&fixtures::notebook(data, person), "Training for a 10k");
    let read = mind(data, person).expect("mind").expect("written");
    assert_eq!(read.revision, revision);
    assert!(read.text.contains("Training for a 10k"), "{}", read.text);
}
