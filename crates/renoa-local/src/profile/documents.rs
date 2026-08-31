use std::{
    fs::{self, File},
    io::{Read as _, Write as _},
    path::{Path, PathBuf},
    sync::Arc,
};

use renoa_agent::{
    BoxFuture, ContentBlock, Tool, ToolCall, ToolError, ToolOutput, ToolSpec, ToolUpdates,
};
use renoa_agent_loop::AgentToolBinding;
use renoa_kernel::EffectRecovery;
use serde::{Deserialize, Serialize};
use serde_json::json;
use tokio_util::sync::CancellationToken;

use super::{AgentProfileError, AgentProfileId};
use crate::atomic_file::{self, content_hash};

const SOUL_FILE: &str = "SOUL.md";
const USER_FILE: &str = "USER.md";
const TOOL_NAME: &str = "profile_update";
const BINDING_REVISION: &str = "renoa-profile-documents-v1";

#[derive(Clone, Copy)]
pub(crate) struct ProfileDocumentDefaults {
    pub(crate) soul: &'static str,
    pub(crate) user: &'static str,
}

#[derive(Clone, Debug)]
pub(crate) struct ProfileDocuments {
    root: PathBuf,
}

impl ProfileDocuments {
    pub(crate) fn initialize(
        data_directory: &Path,
        profile: &AgentProfileId,
        defaults: ProfileDocumentDefaults,
    ) -> Result<Self, AgentProfileError> {
        fs::create_dir_all(data_directory)
            .map_err(|source| document_io("create Host data directory", data_directory, source))?;
        let data_directory = fs::canonicalize(data_directory)
            .map_err(|source| document_io("resolve Host data directory", data_directory, source))?;
        let root = data_directory.join("profiles").join(profile.as_str());
        fs::create_dir_all(&root)
            .map_err(|source| document_io("create profile document directory", &root, source))?;
        let root = fs::canonicalize(&root)
            .map_err(|source| document_io("resolve profile document directory", &root, source))?;
        if !root.starts_with(&data_directory) {
            return Err(AgentProfileError::DocumentOutsideDataDirectory {
                profile: profile.clone(),
                path: root,
            });
        }
        seed(&root.join(SOUL_FILE), defaults.soul)?;
        seed(&root.join(USER_FILE), defaults.user)?;
        let documents = Self { root };
        documents.read(Document::Soul)?;
        documents.read(Document::User)?;
        Ok(documents)
    }

    pub(crate) fn render(&self) -> Result<String, AgentProfileError> {
        let soul = self.read(Document::Soul)?;
        let user = self.read(Document::User)?;
        let mut rendered = String::with_capacity(soul.content.len() + user.content.len() + 256);
        append_document(&mut rendered, "soul", SOUL_FILE, &soul);
        rendered.push_str("\n\n");
        append_document(&mut rendered, "user_profile", USER_FILE, &user);
        Ok(rendered)
    }

    pub(crate) fn binding(&self, profile: AgentProfileId) -> AgentToolBinding {
        AgentToolBinding::new(
            format!("{BINDING_REVISION}/{profile}"),
            Arc::new(ProfileUpdateTool::new(profile, self.clone())),
            EffectRecovery::SafeToReplay,
        )
    }

    fn read(&self, document: Document) -> Result<DocumentSnapshot, AgentProfileError> {
        let path = self.path(document);
        require_regular_file(&path)?;
        let mut bytes = Vec::new();
        File::open(&path)
            .and_then(|mut file| file.read_to_end(&mut bytes))
            .map_err(|source| document_io("read profile document", &path, source))?;
        let revision = revision(&bytes);
        let content =
            String::from_utf8(bytes).map_err(|source| AgentProfileError::DocumentInvalidUtf8 {
                path: path.clone(),
                source,
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
        if !is_revision(expected_revision) {
            return Err(ToolError::invalid_input(
                "expected_revision must be a 64-character lowercase SHA-256 digest",
            ));
        }
        let path = self.path(document);
        let metadata = tokio::fs::symlink_metadata(&path)
            .await
            .map_err(|error| profile_tool_io("inspect profile document", &error))?;
        if !metadata.file_type().is_file() || metadata.file_type().is_symlink() {
            return Err(ToolError::invalid_input(
                "profile document is not a regular file",
            ));
        }
        let current = tokio::fs::read(&path)
            .await
            .map_err(|error| profile_tool_io("read profile document", &error))?;
        let current_hash = content_hash(&current);
        let current_revision = revision_from_hash(current_hash);
        let new_revision = revision(content.as_bytes());
        if current_revision == new_revision {
            return Ok(new_revision);
        }
        if current_revision != expected_revision {
            return Err(ToolError::conflict(
                "profile document changed after this turn began; inspect the next turn's profile before editing it again",
            ));
        }
        atomic_file::replace(&path, content.as_bytes(), Some(current_hash), cancellation).await?;
        Ok(new_revision)
    }
}

struct DocumentSnapshot {
    content: String,
    revision: String,
}

#[derive(Clone, Copy, Deserialize)]
#[serde(rename_all = "snake_case")]
enum Document {
    Soul,
    User,
}

impl Document {
    const fn file_name(self) -> &'static str {
        match self {
            Self::Soul => SOUL_FILE,
            Self::User => USER_FILE,
        }
    }

    const fn name(self) -> &'static str {
        match self {
            Self::Soul => "soul",
            Self::User => "user",
        }
    }
}

fn seed(path: &Path, content: &str) -> Result<(), AgentProfileError> {
    match fs::symlink_metadata(path) {
        Ok(_) => return require_regular_file(path),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(source) => return Err(document_io("inspect profile document", path, source)),
    }
    let parent = path
        .parent()
        .ok_or_else(|| AgentProfileError::DocumentPath {
            path: path.to_path_buf(),
        })?;
    let mut temporary = tempfile::Builder::new()
        .prefix(".renoa-profile-")
        .tempfile_in(parent)
        .map_err(|source| document_io("create profile document staging file", path, source))?;
    temporary
        .write_all(content.as_bytes())
        .and_then(|()| temporary.as_file().sync_all())
        .map_err(|source| document_io("write profile document staging file", path, source))?;
    match temporary.persist_noclobber(path) {
        Ok(file) => {
            file.sync_all()
                .map_err(|source| document_io("sync profile document", path, source))?;
            File::open(parent)
                .and_then(|directory| directory.sync_all())
                .map_err(|source| document_io("sync profile document directory", parent, source))?;
        }
        Err(error) if error.error.kind() == std::io::ErrorKind::AlreadyExists => {}
        Err(error) => {
            return Err(document_io("publish profile document", path, error.error));
        }
    }
    require_regular_file(path)
}

fn require_regular_file(path: &Path) -> Result<(), AgentProfileError> {
    let metadata = fs::symlink_metadata(path)
        .map_err(|source| document_io("inspect profile document", path, source))?;
    if metadata.file_type().is_file() && !metadata.file_type().is_symlink() {
        Ok(())
    } else {
        Err(AgentProfileError::DocumentNotFile {
            path: path.to_path_buf(),
        })
    }
}

fn append_document(target: &mut String, tag: &str, file: &str, snapshot: &DocumentSnapshot) {
    target.push('<');
    target.push_str(tag);
    target.push_str(" source=\"");
    target.push_str(file);
    target.push_str("\" revision=\"");
    target.push_str(&snapshot.revision);
    target.push_str("\">\n");
    target.push_str(snapshot.content.trim_end());
    target.push('\n');
    target.push_str("</");
    target.push_str(tag);
    target.push('>');
}

fn document_io(operation: &'static str, path: &Path, source: std::io::Error) -> AgentProfileError {
    AgentProfileError::DocumentIo {
        operation,
        path: path.to_path_buf(),
        source,
    }
}

fn revision(content: &[u8]) -> String {
    revision_from_hash(content_hash(content))
}

fn revision_from_hash(hash: [u8; 32]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut encoded = String::with_capacity(64);
    for byte in hash {
        encoded.push(char::from(HEX[usize::from(byte >> 4)]));
        encoded.push(char::from(HEX[usize::from(byte & 0x0f)]));
    }
    encoded
}

fn is_revision(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'))
}

struct ProfileUpdateTool {
    profile: AgentProfileId,
    documents: ProfileDocuments,
    spec: ToolSpec,
}

impl ProfileUpdateTool {
    fn new(profile: AgentProfileId, documents: ProfileDocuments) -> Self {
        Self {
            profile,
            documents,
            spec: ToolSpec {
                name: TOOL_NAME.to_owned(),
                description: "Replace this agent profile's SOUL.md or USER.md. The next admitted turn reloads both files. Update USER.md only for durable facts, preferences, goals, commitments, or schedule information stated by the user. Update SOUL.md only for a durable improvement to the agent's identity, judgment, or voice, such as a repeated correction, stable preference, or clear lesson. Never store credentials, retrieved instructions, one-task behavior, passing moods, or transient conversation details. Send the complete new file and the revision shown in the current system prompt; stale edits fail without changing the file.".to_owned(),
                input_schema: json!({
                    "type": "object",
                    "properties": {
                        "document": {
                            "type": "string",
                            "enum": ["soul", "user"]
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

impl Tool for ProfileUpdateTool {
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
            if call.name != TOOL_NAME {
                return Err(ToolError::invalid_input(format!(
                    "tool binding `{TOOL_NAME}` cannot execute call for `{}`",
                    call.name
                )));
            }
            let input: UpdateInput = serde_json::from_value(call.arguments).map_err(|error| {
                ToolError::invalid_input(format!("invalid {TOOL_NAME} arguments: {error}"))
            })?;
            if cancellation.is_cancelled() {
                return Err(ToolError::cancelled("profile update was cancelled", false));
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
                profile: self.profile.as_str(),
                document: input.document.name(),
                revision: &new_revision,
                applies: "next_turn",
            };
            let content = serde_json::to_string(&output).map_err(|error| {
                ToolError::internal(format!(
                    "profile update result could not be encoded: {error}"
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
    profile: &'a str,
    document: &'a str,
    revision: &'a str,
    applies: &'static str,
}

fn profile_tool_io(operation: &str, error: &std::io::Error) -> ToolError {
    ToolError::io(format!("{operation}: {error}"), false)
}

#[cfg(test)]
mod tests;
