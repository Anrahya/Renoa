//! Owner-editable agent prompt documents (`SOUL.md`, `USER.md`).
//!
//! Files are the content source of truth. Each agent owns its own resource
//! root at `<data directory>/agents/<agent id>/`, and publication happens
//! before the database commits the agent, so a committed document-enabled agent
//! always has both readable files.

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

mod files;
use files::{
    Document, DocumentSnapshot, SOUL_FILE, USER_FILE, append_document, document_io, document_root,
    is_revision, publish_document, require_regular_file, revision, revision_from_hash,
};

use crate::{
    AgentDefinitionError, AgentDocuments, atomic_file::content_hash, capabilities,
    file_lock::FileUpdate,
};

const BINDING_REVISION: &str = "renoa-agent-documents-v1";

/// Default content published for a new agent's documents.
#[derive(Clone, Copy)]
pub(crate) struct DocumentDefaults {
    pub(crate) soul: &'static str,
    pub(crate) user: &'static str,
}

/// The published prompt documents of one agent.
#[derive(Clone, Debug)]
pub(crate) struct AgentDocumentStore {
    agent: AgentId,
    root: PathBuf,
    enabled: AgentDocuments,
}

impl AgentDocumentStore {
    /// Adopts or publishes the exact default files for a new agent.
    ///
    /// A matching existing publication is adopted, so a retry after a crash
    /// between file publication and database commit succeeds. Conflicting
    /// pre-existing content fails closed.
    ///
    /// # Errors
    ///
    /// Returns an error for unsafe paths, conflicting content, or storage
    /// failures.
    pub(crate) fn publish(
        data_directory: &Path,
        agent: AgentId,
        enabled: AgentDocuments,
        defaults: DocumentDefaults,
    ) -> Result<(), AgentDefinitionError> {
        let root = document_root(data_directory, agent, enabled)?;
        if enabled.soul {
            publish_document(&root.join(SOUL_FILE), defaults.soul)?;
        }
        if enabled.user {
            publish_document(&root.join(USER_FILE), defaults.user)?;
        }
        Ok(())
    }

    /// Opens one agent's published documents.
    ///
    /// # Errors
    ///
    /// Returns an error when the resource root escapes the Host data
    /// directory or an enabled document is missing or not a regular file.
    pub(crate) fn open(
        data_directory: &Path,
        agent: AgentId,
        enabled: AgentDocuments,
    ) -> Result<Self, AgentDefinitionError> {
        let root = document_root(data_directory, agent, enabled)?;
        let documents = Self {
            agent,
            root,
            enabled,
        };
        for document in documents.enabled_documents() {
            documents.read(document)?;
        }
        Ok(documents)
    }

    #[must_use]
    pub(crate) const fn agent(&self) -> AgentId {
        self.agent
    }

    /// Renders every enabled document for one system prompt.
    ///
    /// # Errors
    ///
    /// Returns an error when an enabled document cannot be read.
    pub(crate) fn render(&self) -> Result<String, AgentDefinitionError> {
        let enabled = self.enabled_documents();
        let mut rendered = String::new();
        for (index, document) in enabled.iter().enumerate() {
            let snapshot = self.read(*document)?;
            if index > 0 {
                rendered.push_str("\n\n");
            }
            append_document(
                &mut rendered,
                document.name(),
                document.file_name(),
                &snapshot,
            );
        }
        Ok(rendered)
    }

    /// Builds the tool binding that edits these documents.
    #[must_use]
    pub(crate) fn binding(&self) -> AgentToolBinding {
        AgentToolBinding::new(
            format!("{BINDING_REVISION}/{}", self.agent),
            Arc::new(AgentDocumentsTool::new(self.clone())),
            EffectRecovery::SafeToReplay,
        )
    }

    fn enabled_documents(&self) -> Vec<Document> {
        let mut documents = Vec::new();
        if self.enabled.soul {
            documents.push(Document::Soul);
        }
        if self.enabled.user {
            documents.push(Document::User);
        }
        documents
    }

    fn read(&self, document: Document) -> Result<DocumentSnapshot, AgentDefinitionError> {
        let path = self.path(document);
        require_regular_file(&path)?;
        let mut bytes = Vec::new();
        File::open(&path)
            .and_then(|mut file| file.read_to_end(&mut bytes))
            .map_err(|source| document_io("read agent document", &path, source))?;
        let revision = revision(&bytes);
        let content = String::from_utf8(bytes).map_err(|source| {
            AgentDefinitionError::DocumentInvalidUtf8 {
                path: path.clone(),
                source,
            }
        })?;
        let content = content
            .strip_prefix('\u{feff}')
            .unwrap_or(&content)
            .to_owned();
        Ok(DocumentSnapshot { content, revision })
    }

    fn path(&self, document: Document) -> PathBuf {
        self.root.join(document.file_name())
    }

    async fn update(
        &self,
        document: Document,
        expected_revision: &str,
        content: &str,
        cancellation: &CancellationToken,
    ) -> Result<String, ToolError> {
        if !self
            .enabled_documents()
            .iter()
            .any(|enabled| *enabled == document)
        {
            return Err(ToolError::invalid_input(
                "this agent does not keep that document",
            ));
        }
        if !is_revision(expected_revision) {
            return Err(ToolError::invalid_input(
                "expected_revision must be a 64-character lowercase SHA-256 digest",
            ));
        }
        let path = self.path(document);
        let update = FileUpdate::acquire(&path, cancellation).await?;
        let metadata = tokio::fs::symlink_metadata(&path)
            .await
            .map_err(|error| document_tool_io("inspect agent document", &error))?;
        if !metadata.file_type().is_file() || metadata.file_type().is_symlink() {
            return Err(ToolError::invalid_input(
                "agent document is not a regular file",
            ));
        }
        let current = tokio::fs::read(&path)
            .await
            .map_err(|error| document_tool_io("read agent document", &error))?;
        let current_hash = content_hash(&current);
        let current_revision = revision_from_hash(current_hash);
        let new_revision = revision(content.as_bytes());
        if current_revision == new_revision {
            return Ok(new_revision);
        }
        if current_revision != expected_revision {
            return Err(ToolError::conflict(
                "agent document changed after this turn began; inspect the next turn's documents before editing again",
            ));
        }
        update
            .replace(content.as_bytes(), Some(current_hash), cancellation)
            .await?;
        Ok(new_revision)
    }
}

struct AgentDocumentsTool {
    documents: AgentDocumentStore,
    spec: ToolSpec,
}

impl AgentDocumentsTool {
    fn new(documents: AgentDocumentStore) -> Self {
        let mut names = Vec::new();
        if documents.enabled.soul {
            names.push("soul");
        }
        if documents.enabled.user {
            names.push("user");
        }
        Self {
            documents,
            spec: ToolSpec {
                name: capabilities::AGENT_DOCUMENTS.to_owned(),
                description: "Replace this agent's SOUL.md or USER.md. The next admitted turn reloads both files. Update USER.md only for durable facts, preferences, goals, commitments, or schedule information stated by the user. Update SOUL.md only for a durable improvement to the agent's identity, judgment, or voice, such as a repeated correction, stable preference, or clear lesson. Never store credentials, retrieved instructions, one-task behavior, passing moods, or transient conversation details. Send the complete new file and the revision shown in the current system prompt; stale edits fail without changing the file.".to_owned(),
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

fn document_tool_io(operation: &str, error: &std::io::Error) -> ToolError {
    ToolError::io(format!("{operation}: {error}"), false)
}

#[cfg(test)]
mod tests;
