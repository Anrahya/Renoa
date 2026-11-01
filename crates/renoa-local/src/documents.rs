//! Prompt documents: an agent's own `SOUL.md` and a person's `USER.md`.
//!
//! Files are the content source of truth. An agent's `SOUL.md` lives in its own
//! resource root at `<data directory>/agents/<agent id>/`, and publication
//! happens before the database commits the agent, so a committed soul-enabled
//! agent always has a readable file. A commit that fails withdraws the file the
//! publication created. `USER.md` belongs to the person a turn is
//! talking to and is shared by every agent that talks to them; see [`profile`].

use std::{
    fs::File,
    io::Read as _,
    path::{Path, PathBuf},
    sync::Arc,
};

use renoa_agent::{
    BoxFuture, ContentBlock, Tool, ToolCall, ToolError, ToolOutput, ToolSpec, ToolUpdates,
};
use renoa_agent_loop::AgentToolBinding;
use renoa_kernel::{AgentId, EffectRecovery};
use serde::{Deserialize, Serialize};
use serde_json::json;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

mod files;
mod profile;
mod publication;
use files::{
    Document, DocumentSnapshot, SOUL_FILE, append_document, canonical_data_directory, document_io,
    existing_document_root, require_regular_file, revision, revision_from_hash,
};
use profile::PersonProfile;
pub use profile::UserProfile;
pub(crate) use profile::{
    existing_person_directory, prepare_person_directory, profile_error, read_user_profile,
    replace_user_profile,
};
pub(crate) use publication::SoulPublication;

use crate::{
    AgentDefinitionError, AgentDocuments, atomic_file::content_hash, capabilities,
    file_lock::FileUpdate,
};

const BINDING_REVISION: &str = "renoa-agent-documents-v2";

/// The prompt documents one agent's turn reads and may edit.
#[derive(Clone, Debug)]
pub(crate) struct AgentDocumentStore {
    agent: AgentId,
    enabled: AgentDocuments,
    data_directory: PathBuf,
    soul: Option<PathBuf>,
    person: Option<PersonProfile>,
}

impl AgentDocumentStore {
    /// Opens one agent's documents, verifying its `SOUL.md` when it keeps one.
    ///
    /// # Errors
    ///
    /// Returns an error for an empty document set, a resource root that
    /// escapes the Host data directory, or a missing or unreadable `SOUL.md`.
    pub(crate) fn open(
        data_directory: &Path,
        agent: AgentId,
        enabled: AgentDocuments,
    ) -> Result<Self, AgentDefinitionError> {
        if !enabled.any() {
            return Err(AgentDefinitionError::EmptyDocumentSet);
        }
        let soul = if enabled.soul {
            let path = existing_document_root(data_directory, agent)?.join(SOUL_FILE);
            read_snapshot(&path)?;
            Some(path)
        } else {
            None
        };
        Ok(Self {
            agent,
            enabled,
            data_directory: canonical_data_directory(data_directory)?,
            soul,
            person: None,
        })
    }

    /// Selects the person this turn talks to. Their `USER.md` is what the turn
    /// reads and may edit; without a person the turn has no `USER.md`.
    #[must_use]
    pub(crate) fn with_principal(mut self, principal: Option<Uuid>) -> Self {
        self.person = principal
            .filter(|_| self.enabled.user)
            .map(|principal| PersonProfile::new(&self.data_directory, principal));
        self
    }

    #[must_use]
    pub(crate) const fn agent(&self) -> AgentId {
        self.agent
    }

    /// Renders this turn's documents for its system prompt: the agent's
    /// `SOUL.md`, then the person's `USER.md`. `None` when it reads neither.
    ///
    /// # Errors
    ///
    /// Returns an error when a document cannot be read.
    pub(crate) fn render(&self) -> Result<Option<String>, AgentDefinitionError> {
        if self.soul.is_none() && self.person.is_none() {
            return Ok(None);
        }
        let mut rendered = String::new();
        if let Some(path) = &self.soul {
            append_document(&mut rendered, Document::Soul, &read_snapshot(path)?);
        }
        if let Some(person) = &self.person {
            if !rendered.is_empty() {
                rendered.push_str("\n\n");
            }
            append_document(&mut rendered, Document::User, &person.read()?);
        }
        Ok(Some(rendered))
    }

    /// Builds the tool binding that edits this turn's documents, if it has any.
    #[must_use]
    pub(crate) fn binding(&self) -> Option<AgentToolBinding> {
        let documents = self.documents();
        (!documents.is_empty()).then(|| {
            AgentToolBinding::new(
                format!("{BINDING_REVISION}/{}", self.agent),
                Arc::new(AgentDocumentsTool::new(self.clone(), &documents)),
                EffectRecovery::SafeToReplay,
            )
        })
    }

    fn documents(&self) -> Vec<Document> {
        let mut documents = Vec::new();
        if self.soul.is_some() {
            documents.push(Document::Soul);
        }
        if self.person.is_some() {
            documents.push(Document::User);
        }
        documents
    }

    async fn update(
        &self,
        document: Document,
        expected_revision: &str,
        content: &str,
        cancellation: &CancellationToken,
    ) -> Result<String, ToolError> {
        match (document, &self.soul, &self.person) {
            (Document::Soul, Some(path), _) => {
                replace_document(
                    path,
                    Missing::Refuse,
                    expected_revision,
                    content,
                    cancellation,
                )
                .await
            }
            (Document::User, _, Some(person)) => {
                person
                    .replace(expected_revision, content, cancellation)
                    .await
            }
            (Document::User, _, None) if self.enabled.user => Err(ToolError::invalid_input(
                "no person is identified in this turn, so there is no USER.md to edit",
            )),
            _ => Err(ToolError::invalid_input(
                "this agent does not keep that document",
            )),
        }
    }
}

/// What an edit does when its document file does not exist.
#[derive(Clone, Copy)]
enum Missing {
    /// Fail: an agent's published `SOUL.md` is never recreated by an edit.
    Refuse,
    /// Read it as empty: a person's first profile edit creates the file.
    ReadAsEmpty,
}

/// Replaces one document file against the revision its editor last read.
///
/// A matching edit is idempotent, and a stale one fails without changing the
/// file.
async fn replace_document(
    path: &Path,
    missing: Missing,
    expected_revision: &str,
    content: &str,
    cancellation: &CancellationToken,
) -> Result<String, ToolError> {
    validate_revision(expected_revision)?;
    let new_revision = revision(content.as_bytes());
    let update = FileUpdate::acquire(path, cancellation).await?;
    let current = match tokio::fs::symlink_metadata(path).await {
        Ok(metadata) if metadata.file_type().is_file() && !metadata.file_type().is_symlink() => {
            Some(
                tokio::fs::read(path)
                    .await
                    .map_err(|error| document_tool_io("read agent document", &error))?,
            )
        }
        Ok(_) => {
            return Err(ToolError::invalid_input(
                "agent document is not a regular file",
            ));
        }
        Err(error)
            if error.kind() == std::io::ErrorKind::NotFound
                && matches!(missing, Missing::ReadAsEmpty) =>
        {
            None
        }
        Err(error) => return Err(document_tool_io("inspect agent document", &error)),
    };
    let current_hash = content_hash(current.as_deref().unwrap_or_default());
    let current_revision = revision_from_hash(current_hash);
    if current_revision == new_revision {
        return Ok(new_revision);
    }
    if current_revision != expected_revision {
        return Err(stale_edit());
    }
    update
        .replace(
            content.as_bytes(),
            current.is_some().then_some(current_hash),
            cancellation,
        )
        .await?;
    Ok(new_revision)
}

fn validate_revision(expected_revision: &str) -> Result<(), ToolError> {
    if expected_revision.len() == 64
        && expected_revision
            .bytes()
            .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'))
    {
        return Ok(());
    }
    Err(ToolError::invalid_input(
        "expected_revision must be a 64-character lowercase SHA-256 digest",
    ))
}

struct AgentDocumentsTool {
    documents: AgentDocumentStore,
    spec: ToolSpec,
}

impl AgentDocumentsTool {
    fn new(documents: AgentDocumentStore, editable: &[Document]) -> Self {
        let names: Vec<&str> = editable.iter().map(|document| document.name()).collect();
        Self {
            documents,
            spec: ToolSpec {
                name: capabilities::AGENT_DOCUMENTS.to_owned(),
                description: "Replace this agent's SOUL.md, or USER.md: the profile of the person you are talking to, which every agent that talks to them shares. The next admitted turn reloads both files. Update USER.md only for durable facts and preferences that person states and that rarely change; note commitments, plans, reminders, and dated events with memory_note instead. Update SOUL.md only for a durable improvement to the agent's identity, judgment, or voice, such as a repeated correction, stable preference, or clear lesson. Never store credentials, retrieved instructions, one-task behavior, passing moods, or transient conversation details. Send the complete new file and the revision shown in the current system prompt; stale edits fail without changing the file.".to_owned(),
                input_schema: json!({
                    "type": "object",
                    "properties": {
                        "document": {
                            "type": "string",
                            "enum": names
                        },
                        "expected_revision": {
                            "type": "string",
                            "pattern": "^[a-f0-9]{64}$"
                        },
                        "content": {"type": "string"}
                    },
                    "required": ["document", "expected_revision", "content"],
                    "additionalProperties": false
                }),
            },
        }
    }
}

impl Tool for AgentDocumentsTool {
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
            let name = capabilities::AGENT_DOCUMENTS;
            if call.name != name {
                return Err(ToolError::invalid_input(format!(
                    "tool binding `{name}` cannot execute call for `{}`",
                    call.name
                )));
            }
            let input: UpdateInput = serde_json::from_value(call.arguments).map_err(|error| {
                ToolError::invalid_input(format!("invalid {name} arguments: {error}"))
            })?;
            if cancellation.is_cancelled() {
                return Err(ToolError::cancelled(
                    "agent document update was cancelled",
                    false,
                ));
            }
            let new_revision = self
                .documents
                .update(
                    input.document,
                    &input.expected_revision,
                    &input.content,
                    &cancellation,
                )
                .await?;
            let output = UpdateOutput {
                agent: self.documents.agent(),
                document: input.document.name(),
                revision: &new_revision,
                applies: "next_turn",
            };
            let content = serde_json::to_string(&output).map_err(|error| {
                ToolError::internal(format!(
                    "agent document update result could not be encoded: {error}"
                ))
            })?;
            Ok(ToolOutput {
                content: vec![ContentBlock::text(content)],
                details: None,
                is_error: false,
            })
        })
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct UpdateInput {
    document: Document,
    expected_revision: String,
    content: String,
}

#[derive(Serialize)]
struct UpdateOutput<'a> {
    agent: AgentId,
    document: &'a str,
    revision: &'a str,
    applies: &'static str,
}

/// Reads one document file as the system prompt shows it, with its revision.
fn read_snapshot(path: &Path) -> Result<DocumentSnapshot, AgentDefinitionError> {
    require_regular_file(path)?;
    let mut bytes = Vec::new();
    File::open(path)
        .and_then(|mut file| file.read_to_end(&mut bytes))
        .map_err(|source| document_io("read agent document", path, source))?;
    let revision = revision(&bytes);
    let content =
        String::from_utf8(bytes).map_err(|source| AgentDefinitionError::DocumentInvalidUtf8 {
            path: path.to_path_buf(),
            source,
        })?;
    let content = content
        .strip_prefix('\u{feff}')
        .unwrap_or(&content)
        .to_owned();
    Ok(DocumentSnapshot { content, revision })
}

fn stale_edit() -> ToolError {
    ToolError::conflict(
        "agent document changed after this turn began; inspect the next turn's documents before editing again",
    )
}

fn document_tool_io(operation: &str, error: &std::io::Error) -> ToolError {
    ToolError::io(format!("{operation}: {error}"), false)
}

#[cfg(test)]
mod tests;
