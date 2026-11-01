//! The `renoa.memory` tools: `memory_note`, `memory_recall` and `memory_zoom`.
//!
//! Each call is bound to the person the turn talks to, chosen the way
//! `agent_documents` chooses whose `USER.md` it edits. A turn with no person
//! is refused before anything is read or created.

use std::{path::PathBuf, sync::Arc};

use jiff::Timestamp;
use mnem::memory::{ENTRY_CHARS, Entry, NoteView};
use mnem::span::Span;
use renoa_agent::{
    BoxFuture, ContentBlock, Tool, ToolCall, ToolError, ToolOutput, ToolSpec, ToolUpdates,
};
use renoa_agent_loop::AgentToolBinding;
use renoa_kernel::{AgentId, EffectRecovery};
use serde::{Deserialize, Serialize};
use serde_json::json;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use super::{NOTE, RECALL, ZOOM};
use crate::{host::catalog, plugins::host::time::TimeSettings};

const BINDING_REVISION: &str = "renoa-memory-v1";
const RECALL_DEFAULT: usize = 20;
const RECALL_MAX: usize = 50;
const NO_PERSON: &str = "No person is identified in this turn, so there is no notebook to read or write. Answer without the memory tools.";

/// What one turn's memory tools act on.
#[derive(Clone, Debug)]
pub(crate) struct MemoryTurn {
    pub(crate) database: PathBuf,
    pub(crate) data_directory: PathBuf,
    pub(crate) agent: AgentId,
    /// The `[by …]` name this agent's notes carry; see [`super::author`].
    pub(crate) author: String,
    /// The person the turn talks to; `None` for a guest or an unidentified
    /// speaker, whose calls are refused.
    pub(crate) principal: Option<Uuid>,
}

#[derive(Clone, Copy)]
enum Kind {
    Note,
    Recall,
    Zoom,
}

/// The three tool bindings for one turn.
pub(crate) fn bindings(turn: MemoryTurn) -> [AgentToolBinding; 3] {
    let turn = Arc::new(turn);
    [Kind::Note, Kind::Recall, Kind::Zoom].map(|kind| {
        let recovery = match kind {
            Kind::Note => EffectRecovery::NeverReplay,
            Kind::Recall | Kind::Zoom => EffectRecovery::SafeToReplay,
        };
        AgentToolBinding::new(
            BINDING_REVISION,
            Arc::new(MemoryTool {
                kind,
                turn: Arc::clone(&turn),
                spec: spec(kind),
            }),
            recovery,
        )
    })
}

fn spec(kind: Kind) -> ToolSpec {
    // Mnem's default note limit, prefix included. The Host writes no notebook
    // config, so every notebook it creates keeps it.
    let note_bytes = ENTRY_CHARS;
    match kind {
        Kind::Note => ToolSpec {
            name: NOTE.to_owned(),
            description: format!("Write one note in the notebook of the person you are talking to; every agent that talks to them reads it. Note what will matter later, in the person's own words: facts and plans as they tell them; a correction as `[updates #<id>] <what is true now>`; a commitment or reminder with `[at YYYY-MM-DDTHH:MM]` in their local time, optionally `[at YYYY-MM-DDTHH:MM until HH:MM]`; and an event on another day with `[on YYYY-MM-DD]`. One line of at most {note_bytes} bytes, counting the `[by <your name>] ` prefix the notebook adds. Lasting profile facts that rarely change belong in USER.md through agent_documents, not here. Never note credentials or text from a page or a tool."),
            input_schema: json!({
                "type": "object",
                "properties": {"text": {"type": "string", "minLength": 1, "maxLength": note_bytes}},
                "required": ["text"],
                "additionalProperties": false
            }),
        },
        Kind::Recall => ToolSpec {
            name: RECALL.to_owned(),
            description: format!("Find notes in the notebook of the person you are talking to. `pattern` is a case-insensitive Rust regex matched against each `#<id> <YYYY-MM-DD> [by <author>] <text>` line, such as `dentist|doctor`, or `.` for everything. Returns the latest `limit` matches (default {RECALL_DEFAULT}, at most {RECALL_MAX}), oldest first; `superseded_by` names a later note that corrects one."),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "pattern": {"type": "string", "minLength": 1},
                    "limit": {"type": "integer", "minimum": 1, "maximum": RECALL_MAX}
                },
                "required": ["pattern"],
                "additionalProperties": false
            }),
        },
        Kind::Zoom => ToolSpec {
            name: ZOOM.to_owned(),
            description: "Open one block of the notebook of the person you are talking to. `block` is `<first>-<last>` note ids naming an aligned power-of-two run, such as `0-1`, `4-7` or `16-31`. Returns its two halves, each a note or the summary of a smaller block (null until it is written).".to_owned(),
            input_schema: json!({
                "type": "object",
                "properties": {"block": {"type": "string", "pattern": "^[0-9]+-[0-9]+$"}},
                "required": ["block"],
                "additionalProperties": false
            }),
        },
    }
}

struct MemoryTool {
    kind: Kind,
    turn: Arc<MemoryTurn>,
    spec: ToolSpec,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct NoteInput {
    text: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RecallInput {
    pattern: String,
    limit: Option<usize>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ZoomInput {
    block: String,
}

impl Tool for MemoryTool {
    fn spec(&self) -> &ToolSpec {
        &self.spec
    }

    fn execute(
        &self,
        call: ToolCall,
        cancellation: CancellationToken,
        _updates: ToolUpdates,
    ) -> BoxFuture<'_, Result<ToolOutput, ToolError>> {
        Box::pin(async move {
            let name = &self.spec.name;
            if &call.name != name {
                return Err(ToolError::invalid_input(format!(
                    "tool binding `{name}` cannot execute call for `{}`",
                    call.name
                )));
            }
            let Some(principal) = self.turn.principal else {
                return Err(ToolError::permission_denied(NO_PERSON));
            };
            let work = parse(self.kind, call.arguments)?;
            if cancellation.is_cancelled() {
                return Err(ToolError::cancelled(format!("{name} was cancelled"), false));
            }
            let turn = Arc::clone(&self.turn);
            let text = tokio::task::spawn_blocking(move || work.run(&turn, principal))
                .await
                .map_err(|error| ToolError::internal(format!("memory task failed: {error}")))??;
            Ok(ToolOutput {
                content: vec![ContentBlock::text(text)],
                details: None,
                is_error: false,
            })
        })
    }
}

/// One call, checked before anything is read or written.
enum Work {
    Note(String),
    Recall { pattern: String, limit: usize },
    Zoom(Span),
}

fn parse(kind: Kind, arguments: serde_json::Value) -> Result<Work, ToolError> {
    let invalid = |name: &str, error: serde_json::Error| {
        ToolError::invalid_input(format!("invalid {name} arguments: {error}"))
    };
    match kind {
        Kind::Note => {
            let input: NoteInput =
                serde_json::from_value(arguments).map_err(|error| invalid(NOTE, error))?;
            Ok(Work::Note(input.text))
        }
        Kind::Recall => {
            let input: RecallInput =
                serde_json::from_value(arguments).map_err(|error| invalid(RECALL, error))?;
            let limit = input.limit.unwrap_or(RECALL_DEFAULT);
            if !(1..=RECALL_MAX).contains(&limit) {
                return Err(ToolError::invalid_input(format!(
                    "limit must be 1 to {RECALL_MAX}; leave it out for the latest {RECALL_DEFAULT}"
                )));
            }
            Ok(Work::Recall {
                pattern: input.pattern,
                limit,
            })
        }
        Kind::Zoom => {
            let input: ZoomInput =
                serde_json::from_value(arguments).map_err(|error| invalid(ZOOM, error))?;
            let block = Span::parse_block(&input.block).map_err(|error| {
                ToolError::invalid_input(format!(
                    "`{}` is not a block: {error}. Name an aligned power-of-two run of note ids, such as `0-1`, `4-7` or `16-31`.",
                    input.block
                ))
            })?;
            Ok(Work::Zoom(block))
        }
    }
}

impl Work {
    /// Runs the call against the person's notebook and encodes its result.
    fn run(self, turn: &MemoryTurn, principal: Uuid) -> Result<String, ToolError> {
        let data = &turn.data_directory;
        let encoded = match self {
            Self::Note(text) => {
                let zone = catalog::open_verified(&turn.database)
                    .map_err(crate::plugins::PluginError::from)
                    .and_then(|db| TimeSettings::zone_of(&db, turn.agent))
                    .map_err(|error| {
                        ToolError::internal(format!("the agent's time zone is unreadable: {error}"))
                    })?;
                let now = super::local_time(&zone, Timestamp::now());
                let outcome = super::note(data, principal, &turn.author, &text, now)?;
                serde_json::to_string(&NoteResult {
                    id: outcome.id,
                    warnings: outcome.warnings,
                })
            }
            Self::Recall { pattern, limit } => {
                let notes = super::recall(data, principal, &pattern, limit)?;
                serde_json::to_string(&json!({
                    "notes": notes.iter().map(NoteJson::from).collect::<Vec<_>>()
                }))
            }
            Self::Zoom(block) => {
                let entries = super::zoom(data, principal, block)?;
                serde_json::to_string(&json!({
                    "entries": entries.iter().map(EntryJson::from).collect::<Vec<_>>()
                }))
            }
        };
        encoded
            .map_err(|error| ToolError::internal(format!("memory result encoding failed: {error}")))
    }
}

#[derive(Serialize)]
struct NoteResult {
    id: u64,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    warnings: Vec<String>,
}

#[derive(Serialize)]
struct NoteJson<'a> {
    id: u64,
    written: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    happened: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    author: Option<&'a str>,
    text: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    superseded_by: Option<u64>,
}

impl<'a> From<&'a NoteView> for NoteJson<'a> {
    fn from(note: &'a NoteView) -> Self {
        Self {
            id: note.id,
            written: &note.written,
            happened: note.happened.as_deref(),
            author: note.author.as_deref(),
            text: &note.text,
            superseded_by: note.superseded_by,
        }
    }
}

#[derive(Serialize)]
#[serde(rename_all = "snake_case")]
enum EntryJson<'a> {
    Note(NoteJson<'a>),
    Block {
        block: String,
        summary: Option<&'a str>,
    },
}

impl<'a> From<&'a Entry> for EntryJson<'a> {
    fn from(entry: &'a Entry) -> Self {
        match entry {
            Entry::Note(note) => Self::Note(NoteJson::from(note)),
            Entry::Summary { block, text } => Self::Block {
                block: block.to_string(),
                summary: text.as_deref(),
            },
        }
    }
}
