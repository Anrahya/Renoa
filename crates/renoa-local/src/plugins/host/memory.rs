//! `renoa.memory`: one Mnem notebook per person, shared by every agent that
//! talks to them.
//!
//! The notebook of a principal lives at `<data directory>/users/<principal
//! id>/mnem/`, beside their `USER.md`. Agents write and read it through the
//! `memory_note`, `memory_recall` and `memory_zoom` tools, and the person's
//! `MIND.md` enters the system prompt through the session-context point. A
//! turn with no person, a guest's or an unidentified speaker's, gets neither.
//!
//! Mnem is synchronous and takes file locks, so every call here blocks and
//! runs off the async executor. It never reads the clock: the Host passes the
//! agent's local time.

use std::{
    fs, io,
    path::{Path, PathBuf},
};

use jiff::{Timestamp, civil::DateTime, tz::TimeZone};
use mnem::memory::{AUTHOR_MAX, Entry, Memory, MemoryError, MindView, NoteOutcome, NoteView};
use mnem::span::Span;
use renoa_agent::ToolError;
use renoa_kernel::AgentId;
use uuid::Uuid;

use crate::documents::{existing_person_directory, prepare_person_directory, profile_error};

#[cfg(test)]
pub(crate) mod fixtures;
mod tools;

pub(crate) use tools::{MemoryTurn, bindings};

pub(crate) const PLUGIN_ID: &str = "renoa.memory";
pub(crate) const DESCRIPTION: &str = "Keep notes about the person you are talking to in a notebook every agent that talks to them shares, read them back, and see what they add up to (MIND.md) in the system prompt.";
pub(crate) const NOTE: &str = "memory_note";
pub(crate) const RECALL: &str = "memory_recall";
pub(crate) const ZOOM: &str = "memory_zoom";

const NOTEBOOK_DIRECTORY: &str = "mnem";
/// A first note is written into a fresh notebook under this prefix and moved
/// into place only once Mnem accepts it, so a refused note leaves nothing.
const STAGING_PREFIX: &str = ".mnem-staging-";
const FALLBACK_AUTHOR_PREFIX: &str = "agent-";

/// The `[by <author>]` name an agent's notes carry: its name without brackets
/// or control characters, trimmed, and cut on a character boundary to Mnem's
/// limit; an agent whose name leaves nothing signs with a short form of its id.
pub(crate) fn author(name: &str, agent: AgentId) -> String {
    let kept: String = name
        .chars()
        .filter(|character| !matches!(character, '[' | ']') && !character.is_control())
        .collect();
    let mut author = kept.trim();
    if author.len() > AUTHOR_MAX {
        author = author[..author.floor_char_boundary(AUTHOR_MAX)].trim_end();
    }
    if author.is_empty() {
        let id = agent.to_string();
        let short: String = id.chars().filter(char::is_ascii_hexdigit).take(8).collect();
        return format!("{FALLBACK_AUTHOR_PREFIX}{short}");
    }
    author.to_owned()
}

/// The local wall time Mnem dates a note with: `at` in `zone`.
pub(crate) fn local_time(zone: &TimeZone, at: Timestamp) -> DateTime {
    at.to_zoned(zone.clone()).datetime()
}

/// Writes one note in the notebook of `principal`, creating the notebook with
/// its first note.
fn note(
    data_directory: &Path,
    principal: Uuid,
    author: &str,
    text: &str,
    now: DateTime,
) -> Result<NoteOutcome, ToolError> {
    // Mnem checks the note with its `[by …]` prefix, which is never blank, so
    // a blank note would be kept as an author with nothing said.
    if text.trim().is_empty() {
        return Err(ToolError::invalid_input(
            "The note is empty; write what to remember as one line of text.",
        ));
    }
    if let Some(notebook) = existing_notebook(data_directory, principal)? {
        return open(&notebook)?
            .note(author, text, now)
            .map_err(|error| tool_error(error, true));
    }
    first_note(data_directory, principal, author, text, now)
}

/// The latest `limit` notes matching `pattern`; none when the person has no
/// notebook, which is not created.
fn recall(
    data_directory: &Path,
    principal: Uuid,
    pattern: &str,
    limit: usize,
) -> Result<Vec<NoteView>, ToolError> {
    let Some(notebook) = existing_notebook(data_directory, principal)? else {
        return Ok(Vec::new());
    };
    open(&notebook)?
        .recall(pattern, limit)
        .map_err(|error| tool_error(error, false))
}

/// The halves of `block`; none when the person has no notebook, which is not
/// created.
fn zoom(data_directory: &Path, principal: Uuid, block: Span) -> Result<Vec<Entry>, ToolError> {
    let Some(notebook) = existing_notebook(data_directory, principal)? else {
        return Ok(Vec::new());
    };
    open(&notebook)?
        .zoom(block)
        .map_err(|error| tool_error(error, false))
}

/// The person's `MIND.md`, when they have a notebook and Mnem's Dream wrote
/// one. Nothing is created.
pub(crate) fn mind(data_directory: &Path, principal: Uuid) -> Result<Option<MindView>, String> {
    let Some(notebook) =
        existing_notebook(data_directory, principal).map_err(|error| error.to_string())?
    else {
        return Ok(None);
    };
    Memory::open(&notebook)
        .and_then(|memory| memory.mind())
        .map_err(|error| error.to_string())
}

/// The person's notebook directory, if it exists. A link or another file in
/// its place is refused, never followed.
fn existing_notebook(data_directory: &Path, principal: Uuid) -> Result<Option<PathBuf>, ToolError> {
    let Some(person) = existing_person_directory(data_directory, principal)
        .map_err(|error| profile_error(&error))?
    else {
        return Ok(None);
    };
    let notebook = person.join(NOTEBOOK_DIRECTORY);
    match fs::symlink_metadata(&notebook) {
        Ok(metadata) if metadata.is_dir() => Ok(Some(notebook)),
        Ok(_) => Err(ToolError::invalid_input(
            "this person's notebook is not a plain directory; the Host owner must repair it",
        )),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(ToolError::io(
            format!("this person's notebook could not be inspected: {error}"),
            false,
        )),
    }
}

/// Creates the person's notebook with its first note. The note is written into
/// a fresh notebook beside the final one and renamed into place, so a note
/// Mnem refuses leaves no notebook and no directory this call created. Two
/// agents writing a person's first note at once both succeed: the one that
/// renames second writes its note into the first one's notebook.
fn first_note(
    data_directory: &Path,
    principal: Uuid,
    author: &str,
    text: &str,
    now: DateTime,
) -> Result<NoteOutcome, ToolError> {
    let prepared = prepare_person_directory(data_directory, principal)
        .map_err(|error| profile_error(&error))?;
    let person = prepared.directory();
    let staging = person.join(format!("{STAGING_PREFIX}{}", Uuid::new_v4()));
    let written = Memory::create(&staging).and_then(|memory| memory.note(author, text, now));
    let outcome = match written {
        Ok(outcome) => outcome,
        Err(error) => {
            discard(&staging);
            prepared.withdraw();
            return Err(tool_error(error, false));
        }
    };
    let notebook = person.join(NOTEBOOK_DIRECTORY);
    match fs::rename(&staging, &notebook) {
        Ok(()) => {
            sync_directory(person)?;
            Ok(outcome)
        }
        Err(_) if fs::symlink_metadata(&notebook).is_ok_and(|metadata| metadata.is_dir()) => {
            discard(&staging);
            open(&notebook)?
                .note(author, text, now)
                .map_err(|error| tool_error(error, true))
        }
        Err(error) => {
            discard(&staging);
            prepared.withdraw();
            Err(ToolError::io(
                format!("this person's notebook could not be created: {error}"),
                false,
            ))
        }
    }
}

/// Removes a staged notebook no one else can see. Best effort: it is hidden
/// and holds no note any agent was told was kept.
fn discard(staging: &Path) {
    let _ = fs::remove_dir_all(staging);
}

fn sync_directory(directory: &Path) -> Result<(), ToolError> {
    fs::File::open(directory)
        .and_then(|handle| handle.sync_all())
        .map_err(|error| {
            ToolError::io(
                format!(
                    "the note was written, but its new notebook could not be made durable ({error}); recall it before noting it again"
                ),
                true,
            )
        })
}

fn open(notebook: &Path) -> Result<Memory, ToolError> {
    Memory::open(notebook).map_err(|error| tool_error(error, false))
}

/// Mnem's error as the agent sees it. Invalid input carries Mnem's own advice
/// on the correct call; storage details stay out of the model's view.
fn tool_error(error: MemoryError, writing: bool) -> ToolError {
    match error {
        MemoryError::Invalid { message } => ToolError::invalid_input(message),
        MemoryError::Conflict { message } => ToolError::conflict(message),
        MemoryError::Corrupt { .. } => ToolError::internal(
            "this person's notebook cannot be read as Mnem wrote it; the Host owner must repair it",
        ),
        MemoryError::Io { source, .. } => ToolError::io(
            format!("this person's notebook could not be read or written: {source}"),
            writing,
        ),
    }
}

#[cfg(test)]
mod tests;
