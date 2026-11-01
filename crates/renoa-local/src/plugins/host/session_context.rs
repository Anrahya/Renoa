//! The `session_context` point: attributed blocks a plugin adds to the system
//! prompt of a turn that talks to a person.
//!
//! The system prompt is the cached prefix of every model request, so a block
//! changes only when its source does: each block carries its source's revision,
//! and the prompt's bytes change exactly when a revision does. A block that
//! changes with every write, such as recent notes, does not belong here.
//!
//! Blocks are read once, when a command first resolves its runtime, from each
//! plugin enabled for the agent, and pinned to that command in its session
//! directory before it is admitted. A retried or recovered command reuses the
//! pin, so it builds the prompt the kernel froze into its manifest even after a
//! source changed or a plugin was turned off. A command that is already
//! admitted without a pin was admitted with no block and keeps none. A
//! contributor that fails or exceeds the budget is left out and reported, and
//! never blocks the turn; what a block holds is its plugin's policy.
//!
//! A guest's or an unidentified speaker's turn has no person and gets no block.
//! The contributors are compiled plugins only, matched exhaustively.

use std::{
    borrow::Cow,
    fs,
    io::{self, Write as _},
    path::Path,
};

use renoa_kernel::{AgentId, CommandId, CommandStanding};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use super::{HostPluginId, Skipped, memory, state};
use crate::host::catalog;

#[cfg(test)]
pub(crate) mod pin_fault;

/// The file in a session directory that holds the latest pinned command.
pub(crate) const PIN_FILE: &str = "session-context.json";
const STAGED_PIN_FILE: &str = "session-context.json.staged";

/// What one plugin contributes: its block's text and the revision of the
/// source it was read from.
pub(crate) struct Contribution {
    pub(crate) revision: String,
    pub(crate) text: String,
}

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

impl SessionBlock {
    /// The block's text as the prompt carries it.
    fn rendered_text(&self) -> String {
        escape(self.text.trim_end())
    }
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
            rendered.push_str(&block.rendered_text());
            rendered.push_str("\n</context>");
        }
        rendered.push_str("\n</session_context>");
        Some(rendered)
    }
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
        // The pin holds the blocks, so the command carries them too: a retry
        // reads the same pin.
        Err(PinFailure::Undurable(error)) => {
            skipped.push(Skipped {
                source: "*",
                reason: format!("the session context was pinned but not made durable: {error}"),
            });
            (context, skipped)
        }
        // No pin names this command, so it carries no block, as a retry finds.
        Err(PinFailure::Unwritten(error)) => {
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
        let contributed = state::enabled_in(&db, request.agent, plugin)
            .map_err(|error| error.to_string())
            .and_then(|enabled| {
                if enabled {
                    contribution(&db, request, plugin)
                } else {
                    Ok(None)
                }
            });
        let block = contributed.map(|found| {
            found.map(|contribution| SessionBlock {
                source: format!("plugin:{}", plugin.id()),
                revision: contribution.revision,
                text: contribution.text,
            })
        });
        match block {
            Ok(Some(block)) => {
                let (bytes, most) = (block.rendered_text().len(), budget(plugin));
                if bytes > most {
                    skipped.push(Skipped {
                        source: plugin.id(),
                        reason: format!(
                            "its block is {bytes} bytes as rendered; its plugin's blocks hold at most {most}"
                        ),
                    });
                } else {
                    blocks.push(block);
                }
            }
            Ok(None) => {}
            Err(reason) => skipped.push(Skipped {
                source: plugin.id(),
                reason,
            }),
        }
    }
    SessionContext { blocks }
}

/// One enabled plugin's contribution, or none when it adds nothing for this
/// person.
fn contribution(
    db: &rusqlite::Connection,
    request: &ContextRequest<'_>,
    plugin: HostPluginId,
) -> Result<Option<Contribution>, String> {
    match plugin {
        HostPluginId::Memory => {
            memory::session_block(db, request.data_directory, request.agent, request.principal)
        }
        HostPluginId::Agents
        | HostPluginId::Automations
        | HostPluginId::Documents
        | HostPluginId::Skills
        | HostPluginId::Git
        | HostPluginId::Time => Ok(None),
    }
}

/// The largest block `plugin` may add, as the prompt carries it: escaped, with
/// its heading. Each contributor declares its own from what its source can
/// hold; a block over it is left out instead of filling the prompt. A plugin
/// that contributes nothing has none.
const fn budget(plugin: HostPluginId) -> usize {
    match plugin {
        HostPluginId::Memory => memory::SESSION_BLOCK_MAX,
        HostPluginId::Agents
        | HostPluginId::Automations
        | HostPluginId::Documents
        | HostPluginId::Skills
        | HostPluginId::Git
        | HostPluginId::Time => 0,
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

/// Why a pin was not written durably.
enum PinFailure {
    /// The pin was not replaced; the previous one, if any, stands.
    Unwritten(io::Error),
    /// The pin was replaced, but a crash could still lose the replacement.
    Undurable(io::Error),
}

/// Replaces the pin atomically: a crash leaves the old pin or the new one,
/// never a part, and a failure before the replacement leaves no staged file.
fn write_pin(path: &Path, command: CommandId, context: &SessionContext) -> Result<(), PinFailure> {
    let directory = path
        .parent()
        .ok_or_else(|| PinFailure::Unwritten(io::Error::other("the pin has no directory")))?;
    let staged = directory.join(STAGED_PIN_FILE);
    let replaced = stage(&staged, command, context).and_then(|()| {
        #[cfg(test)]
        pin_fault::inject(path, pin_fault::Stage::BeforeRename)?;
        fs::rename(&staged, path)
    });
    if let Err(error) = replaced {
        // Best effort: the staged file is never read, and the next write
        // replaces it.
        let _ = fs::remove_file(&staged);
        return Err(PinFailure::Unwritten(error));
    }
    #[cfg(test)]
    pin_fault::inject(path, pin_fault::Stage::AfterRename).map_err(PinFailure::Undurable)?;
    fs::File::open(directory)
        .and_then(|handle| handle.sync_all())
        .map_err(PinFailure::Undurable)
}

fn stage(staged: &Path, command: CommandId, context: &SessionContext) -> io::Result<()> {
    let bytes = serde_json::to_vec(&Pin {
        command_id: command,
        context: Cow::Borrowed(context),
    })?;
    let mut file = fs::File::create(staged)?;
    file.write_all(&bytes)?;
    file.sync_all()
}

/// Block text is data: it can never open or close a tag of its own.
fn escape(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

#[cfg(test)]
mod tests;
