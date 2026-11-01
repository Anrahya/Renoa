//! The `session_context` point: attributed blocks a plugin adds to the system
//! prompt of a turn that talks to a person.
//!
//! The system prompt is the cached prefix of every model request, so a block
//! changes only when its source does: each block carries its source's revision,
//! and the prompt's bytes change exactly when a revision does. A block that
//! changes with every write, such as recent notes, does not belong here.
//!
//! Blocks are read once, when a command first resolves its runtime, and pinned
//! to that command in its session directory before it is admitted. A retried or
//! recovered command reuses the pin, so it builds the prompt the kernel froze
//! into its manifest even after the source changed or the plugin was turned
//! off, or the Host's main agent changed. A command that is already admitted
//! without a pin was admitted with no
//! block and keeps none. A contributor that fails or exceeds the budget is left
//! out and reported, and never blocks the turn.
//!
//! A guest's or an unidentified speaker's turn has no person and gets no block.
//! The contributors are compiled plugins only, matched exhaustively.

use std::{
    borrow::Cow,
    fs,
    io::{self, Write as _},
    path::Path,
};

use renoa_kernel::{AgentId, CommandId};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use super::{HostPluginId, Skipped, memory, state};
use crate::host::{catalog, definition::is_main_agent};

/// The file in a session directory that holds the latest pinned command.
pub(crate) const PIN_FILE: &str = "session-context.json";

/// The largest text one block may carry, in bytes. Mnem aims `MIND.md` at about
/// 2k tokens (8 KiB); its own caps, 75 lines of at most 400 bytes, allow about
/// 30 KiB, so every file Mnem can write fits with its heading line, and a larger
/// one, which Mnem did not write, is left out instead of filling the prompt.
pub(crate) const MAX_BLOCK_BYTES: usize = 32 * 1024;

/// The line `renoa.memory`'s block opens with, ahead of `MIND.md` itself.
const MIND_HEADING: &str = "MIND.md: what the shared notebook of the person you are talking to adds up to. Use memory_recall for the notes behind it.";

/// The blocks one command's system prompt carries, in contributor order.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub(crate) struct SessionContext {
    blocks: Vec<SessionBlock>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct SessionBlock {
    source: String,
    revision: String,
    text: String,
}

impl SessionContext {
    /// The `<session_context>` element for the system prompt, `None` when there
    /// is no block. Text is escaped: it can never open or close a tag.
    pub(crate) fn render(&self) -> Option<String> {
        if self.blocks.is_empty() {
            return None;
        }
        let mut rendered = String::from("<session_context>");
        for block in &self.blocks {
            rendered.push_str("\n<context source=\"");
            rendered.push_str(&block.source);
            rendered.push_str("\" revision=\"");
            rendered.push_str(&block.revision);
            rendered.push_str("\">\n");
            rendered.push_str(&escape(block.text.trim_end()));
            rendered.push_str("\n</context>");
        }
        rendered.push_str("\n</session_context>");
        Some(rendered)
    }
}

/// Where a command stands in its session, read from the kernel before its
/// runtime is resolved.
#[derive(Clone, Copy, Debug)]
pub(crate) struct CommandStanding {
    /// The kernel already holds this command.
    pub(crate) admitted: bool,
    /// Another command is unfinished, so this one cannot be admitted now.
    pub(crate) other_unfinished: bool,
}

/// What one command's session context is resolved from.
pub(crate) struct ContextRequest<'a> {
    pub(crate) database: &'a Path,
    pub(crate) data_directory: &'a Path,
    pub(crate) agent: AgentId,
    pub(crate) principal: Uuid,
    pub(crate) command: CommandId,
    pub(crate) standing: CommandStanding,
    /// This session's [`PIN_FILE`].
    pub(crate) pin: &'a Path,
}

/// The session context for one command, and the contributors left out of it.
pub(crate) fn resolve(request: &ContextRequest<'_>) -> (SessionContext, Vec<Skipped>) {
    let mut skipped = Vec::new();
    match read_pin(request.pin) {
        Ok(Some(pin)) if pin.command_id == request.command => {
            return (pin.context.into_owned(), skipped);
        }
        Ok(_) => {}
        Err(reason) => skipped.push(Skipped {
            source: "*",
            reason,
        }),
    }
    if request.standing.admitted {
        return (SessionContext::default(), skipped);
    }
    let context = contributions(request, &mut skipped);
    // An empty context needs no pin: the command, once admitted, has none and
    // resolves to none again. One that cannot be admitted keeps the unfinished
    // command's pin in place.
    if context.blocks.is_empty() || request.standing.other_unfinished {
        return (context, skipped);
    }
    match write_pin(request.pin, request.command, &context) {
        Ok(()) => (context, skipped),
        Err(error) => {
            skipped.push(Skipped {
                source: "*",
                reason: format!("the session context could not be pinned: {error}"),
            });
            (SessionContext::default(), skipped)
        }
    }
}

fn contributions(request: &ContextRequest<'_>, skipped: &mut Vec<Skipped>) -> SessionContext {
    let db = match catalog::open_verified(request.database) {
        Ok(db) => db,
        Err(error) => {
            skipped.push(Skipped {
                source: "*",
                reason: error.to_string(),
            });
            return SessionContext::default();
        }
    };
    let mut blocks = Vec::new();
    for plugin in HostPluginId::ALL {
        match contribution(&db, request, plugin) {
            Ok(Some(block)) if block.text.len() > MAX_BLOCK_BYTES => skipped.push(Skipped {
                source: plugin.id(),
                reason: format!(
                    "its block is {} bytes; a session context block holds at most {MAX_BLOCK_BYTES}",
                    block.text.len()
                ),
            }),
            Ok(Some(block)) => blocks.push(block),
            Ok(None) => {}
            Err(reason) => skipped.push(Skipped {
                source: plugin.id(),
                reason,
            }),
        }
    }
    SessionContext { blocks }
}

/// One plugin's block, or none when it adds nothing for this person.
fn contribution(
    db: &rusqlite::Connection,
    request: &ContextRequest<'_>,
    plugin: HostPluginId,
) -> Result<Option<SessionBlock>, String> {
    match plugin {
        HostPluginId::Memory => {
            if !state::enabled_in(db, request.agent, plugin).map_err(|error| error.to_string())? {
                return Ok(None);
            }
            let Some(mind) = memory::mind(request.data_directory, request.principal)? else {
                return Ok(None);
            };
            // Nudges are what to do for the person and when; only the main
            // agent acts on them. Each rendering carries its own revision, so
            // a nudge-only change leaves every other agent's prompt as it was.
            let (revision, text) =
                if is_main_agent(db, request.agent).map_err(|error| error.to_string())? {
                    (mind.revision, mind.text)
                } else {
                    let shared = mind.without_nudges();
                    (shared.revision, shared.text)
                };
            Ok(Some(SessionBlock {
                source: format!("plugin:{}", plugin.id()),
                revision,
                text: format!("{MIND_HEADING}\n{text}"),
            }))
        }
        HostPluginId::Agents
        | HostPluginId::Automations
        | HostPluginId::Documents
        | HostPluginId::Skills
        | HostPluginId::Git
        | HostPluginId::Time => Ok(None),
    }
}

/// The context one command resolved, as the session directory keeps it.
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Pin<'a> {
    command_id: CommandId,
    context: Cow<'a, SessionContext>,
}

fn read_pin(path: &Path) -> Result<Option<Pin<'static>>, String> {
    let bytes = match fs::read(path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(format!("the session context pin is unreadable: {error}")),
    };
    serde_json::from_slice(&bytes)
        .map(Some)
        .map_err(|error| format!("the session context pin is invalid: {error}"))
}

/// Replaces the pin atomically and durably: a crash leaves the old pin or the
/// new one, never a part.
fn write_pin(path: &Path, command: CommandId, context: &SessionContext) -> io::Result<()> {
    let bytes = serde_json::to_vec(&Pin {
        command_id: command,
        context: Cow::Borrowed(context),
    })?;
    let directory = path
        .parent()
        .ok_or_else(|| io::Error::other("the session context pin has no directory"))?;
    let staged = directory.join(format!("{PIN_FILE}.staged"));
    let mut file = fs::File::create(&staged)?;
    file.write_all(&bytes)?;
    file.sync_all()?;
    drop(file);
    fs::rename(&staged, path)?;
    fs::File::open(directory)?.sync_all()
}

/// Block text is data: it can never open or close a tag of its own.
fn escape(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

#[cfg(test)]
mod tests;
