use std::{fs, path::Path};

use jiff::{Timestamp, civil::DateTime, tz::TimeZone};
use mnem::memory::{AUTHOR_MAX, ENTRY_CHARS, Memory, MemoryError, strip_author};
use mnem::span::Span;
use renoa_agent::ToolErrorCode;
use renoa_kernel::AgentId;
use uuid::Uuid;

use super::{author, fixtures, local_time, mind, note, recall, tool_error, zoom};

thread_local! {
    /// The directories this thread synced, in order.
    pub(super) static SYNCED: std::cell::RefCell<Vec<std::path::PathBuf>> =
        const { std::cell::RefCell::new(Vec::new()) };
    /// Runs once just before Mnem writes a note, as another call would.
    pub(super) static BEFORE_WRITE: std::cell::RefCell<Option<Box<dyn FnOnce()>>> =
        const { std::cell::RefCell::new(None) };
    /// Runs once just after Mnem kept a note.
    pub(super) static AFTER_WRITE: std::cell::RefCell<Option<Box<dyn FnOnce()>>> =
        const { std::cell::RefCell::new(None) };
    /// A directory whose sync this thread fails, after it really synced.
    static SYNC_FAULT: std::cell::RefCell<Option<std::path::PathBuf>> =
        const { std::cell::RefCell::new(None) };
}

/// The injected failure for a sync of `directory`, when one is armed.
pub(super) fn sync_fault(directory: &Path) -> std::io::Result<()> {
    if SYNC_FAULT.with_borrow(|fault| fault.as_deref() == Some(directory)) {
        return Err(std::io::Error::other("injected sync failure"));
    }
    Ok(())
}

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
        "\u{1c}\u{1f}",
        "two\nlines",
        "[updates #0] corrects nothing yet",
        &"x".repeat(ENTRY_CHARS),
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
    assert_eq!(entries, ["mnem"], "only the notebook is created");
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
fn a_refused_note_leaves_an_existing_person_directory_as_it_was() {
    let directory = tempfile::tempdir().expect("data directory");
    let data = directory.path();
    let person = Uuid::new_v4();
    let now = at("2026-10-31T09:00");
    let person_directory = users(data).join(person.to_string());
    fs::create_dir_all(&person_directory).expect("person directory");
    fs::write(person_directory.join("USER.md"), "Lives near the lake.\n").expect("USER.md");

    let refused = note(data, person, "Arcee", "two\nlines", now);
    assert!(refused.is_err());
    assert!(
        !person_directory.join("mnem").exists(),
        "Mnem checks a first note before it creates the notebook"
    );
    assert!(
        person_directory.join("USER.md").is_file(),
        "the directory stays"
    );

    // A notebook directory without a log is not a notebook yet: reads find
    // nothing and create nothing, and the first kept note makes it one.
    let notebook = person_directory.join("mnem");
    fs::create_dir(&notebook).expect("an unfinished notebook");
    assert!(recall(data, person, ".", 20).expect("recall").is_empty());
    assert_eq!(mind(data, person).expect("mind"), None);
    assert_eq!(
        fs::read_dir(&notebook).expect("notebook").count(),
        0,
        "a read creates nothing"
    );
    assert!(note(data, person, "Arcee", "", now).is_err());
    assert_eq!(fs::read_dir(&notebook).expect("notebook").count(), 0);
    let kept = note(data, person, "Arcee", "Walks every evening", now).expect("kept");
    assert_eq!(kept.id, 0);
    assert_eq!(recall(data, person, ".", 20).expect("recall").len(), 1);
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

#[test]
fn a_busy_notebook_is_unavailable_and_a_stale_job_a_conflict() {
    let busy = tool_error(
        MemoryError::Busy {
            message: "another dream is running".to_owned(),
        },
        false,
    );
    assert_eq!(busy.code(), ToolErrorCode::Unavailable, "ask again later");
    let stale = tool_error(
        MemoryError::Conflict {
            message: "the job is stale".to_owned(),
        },
        false,
    );
    assert_eq!(stale.code(), ToolErrorCode::Conflict);
}

#[cfg(unix)]
#[test]
fn a_note_keeps_the_person_directory_private_and_every_one_durable() {
    use std::os::unix::fs::PermissionsExt as _;

    let directory = tempfile::tempdir().expect("data directory");
    let data = directory.path();
    let person = Uuid::new_v4();
    let now = at("2026-10-31T09:00");
    let person_directory = users(data).join(person.to_string());
    let open = |path: &Path| {
        fs::set_permissions(path, fs::Permissions::from_mode(0o755)).expect("open it up");
    };
    let mode = |path: &Path| fs::metadata(path).expect("mode").permissions().mode() & 0o777;
    // A directory an interrupted run left with default modes.
    fs::create_dir_all(&person_directory).expect("person directory");
    open(&users(data));
    open(&person_directory);

    SYNCED.with_borrow_mut(Vec::clear);
    note(data, person, "Arcee", "Walks every evening", now).expect("first note");
    assert_eq!(mode(&person_directory), 0o700);
    assert_eq!(mode(&users(data)), 0o700);
    let canonical = data.canonicalize().expect("canonical");
    let person_canonical = canonical.join("users").join(person.to_string());
    let ancestors = [person_canonical, canonical.join("users"), canonical.clone()];
    assert_eq!(
        SYNCED.with_borrow(Clone::clone),
        ancestors,
        "the new notebook's entry and every parent above it"
    );

    open(&person_directory);
    SYNCED.with_borrow_mut(Vec::clear);
    note(data, person, "Arcee", "Swims on Sundays", now).expect("second note");
    assert_eq!(
        mode(&person_directory),
        0o700,
        "restricted after every note"
    );
    assert_eq!(
        SYNCED.with_borrow(Clone::clone),
        ancestors,
        "a later note cannot know an earlier one's syncs finished"
    );

    // Another call's refused first note withdraws the directory after this one
    // prepared it, and Mnem re-creates it with default modes.
    let raced = Uuid::new_v4();
    let raced_directory = canonical.join("users").join(raced.to_string());
    let gone = raced_directory.clone();
    BEFORE_WRITE.with_borrow_mut(|hook| {
        *hook = Some(Box::new(move || {
            fs::remove_dir(&gone).expect("the other call withdrew it");
        }));
    });
    note(data, raced, "Arcee", "Reads before bed", now).expect("raced note");
    assert_eq!(mode(&raced_directory), 0o700, "restricted after the note");
}

/// A failure after Mnem kept a note says the note may be written, so the
/// agent recalls before it notes again instead of writing it twice.
fn assert_kept(error: &renoa_agent::ToolError, what: &str) {
    assert_eq!(error.code(), ToolErrorCode::Io, "{error}");
    assert!(error.partial_changes_possible(), "{error}");
    let text = error.to_string();
    assert!(text.starts_with("the note was written, but "), "{text}");
    assert!(text.contains(what), "{text}");
    assert!(text.ends_with("recall it before noting it again"), "{text}");
}

#[cfg(unix)]
#[test]
fn a_failure_after_the_note_is_kept_says_it_was_written() {
    let directory = tempfile::tempdir().expect("data directory");
    let data = directory.path();
    let person = Uuid::new_v4();
    let now = at("2026-10-31T09:00");
    let canonical = data.canonicalize().expect("canonical");
    let person_directory = canonical.join("users").join(person.to_string());
    let ancestors = [
        person_directory.clone(),
        canonical.join("users"),
        canonical.clone(),
    ];

    SYNC_FAULT.with_borrow_mut(|fault| *fault = Some(person_directory.clone()));
    let unsynced = note(data, person, "Arcee", "Walks every evening", now)
        .expect_err("the first note's directory sync fails");
    SYNC_FAULT.with_borrow_mut(|fault| *fault = None);
    assert_kept(&unsynced, "could not be made durable");
    assert_eq!(recall(data, person, ".", 20).expect("recall").len(), 1);

    SYNCED.with_borrow_mut(Vec::clear);
    note(data, person, "Arcee", "Swims on Sundays", now).expect("second note");
    assert_eq!(
        SYNCED.with_borrow(Clone::clone),
        ancestors,
        "the next note makes the first one's entries durable"
    );

    // The person's directory turns into a link after Mnem kept the note: the
    // syncs run first, then restricting refuses the link.
    let moved = canonical.join("users").join("moved");
    let (from, to) = (person_directory.clone(), moved.clone());
    AFTER_WRITE.with_borrow_mut(|hook| {
        *hook = Some(Box::new(move || {
            fs::rename(&from, &to).expect("move the directory");
            std::os::unix::fs::symlink(&to, &from).expect("link it back");
        }));
    });
    SYNCED.with_borrow_mut(Vec::clear);
    let unrestricted = note(data, person, "Arcee", "Reads before bed", now)
        .expect_err("restricting refuses the link");
    assert_kept(&unrestricted, "could not be kept private");
    assert_eq!(
        SYNCED.with_borrow(Clone::clone),
        ancestors,
        "durable before the directory is restricted"
    );
    let log = fs::read_to_string(moved.join("mnem").join("LOG.txt")).expect("log");
    assert!(log.contains("Reads before bed"), "{log}");
}
