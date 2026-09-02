use std::{
    fmt,
    fs::{self, File},
    io::{self, Read as _},
    num::NonZeroU64,
    path::{Path, PathBuf},
    str::FromStr,
};

use serde::{Deserialize, Deserializer, Serialize, Serializer};
use thiserror::Error;

use crate::ModelProvider;

mod documents;
#[cfg(test)]
mod tests;

pub(crate) use documents::{ProfileDocumentDefaults, ProfileDocuments};

const MAX_PROFILE_ID_BYTES: usize = 128;
const PROJECT_INSTRUCTIONS_FILE: &str = "AGENTS.md";
const MAX_PROJECT_INSTRUCTIONS_BYTES: usize = 32 * 1024;
const MAX_PROJECT_INSTRUCTIONS_BYTES_U64: u64 = 32 * 1024;

/// Stable Host identity of one reusable agent recipe.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct AgentProfileId(String);

impl AgentProfileId {
    /// Validates a profile identity before it reaches Host storage.
    ///
    /// # Errors
    ///
    /// Returns an error for an empty, oversized, or non-portable identifier.
    pub fn new(value: impl Into<String>) -> Result<Self, AgentProfileError> {
        let value = value.into();
        if value.is_empty()
            || value.len() > MAX_PROFILE_ID_BYTES
            || !value
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-' | b'.'))
        {
            return Err(AgentProfileError::InvalidId);
        }
        Ok(Self(value))
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for AgentProfileId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

impl FromStr for AgentProfileId {
    type Err = AgentProfileError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        Self::new(value)
    }
}

impl Serialize for AgentProfileId {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(self.as_str())
    }
}

impl<'de> Deserialize<'de> for AgentProfileId {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        Self::new(value).map_err(serde::de::Error::custom)
    }
}

/// Declarative Host recipe shared by every instance created from it.
#[derive(Clone)]
pub struct AgentProfile {
    id: AgentProfileId,
    base_instructions: String,
    documents: Option<ProfileDocuments>,
    model_provider: Option<ModelProvider>,
    workspace_instructions: WorkspaceInstructions,
    turn_timing: TurnTimingPolicy,
    automatic_compaction: Option<AutomaticCompactionPolicy>,
}

#[derive(Clone, Copy)]
pub(crate) struct AutomaticCompactionPolicy {
    pub(crate) trigger_input_tokens: NonZeroU64,
    pub(crate) target_input_tokens: NonZeroU64,
}

#[derive(Clone, Copy)]
enum WorkspaceInstructions {
    None,
    RootAgentsFile,
}

#[derive(Clone, Copy)]
enum TurnTimingPolicy {
    None,
    HostClock,
}

impl AgentProfile {
    /// Creates a profile with fixed base instructions and no project rules.
    ///
    /// # Errors
    ///
    /// Returns an error when the identity or base instructions are invalid.
    pub fn new(
        id: impl Into<String>,
        base_instructions: impl Into<String>,
    ) -> Result<Self, AgentProfileError> {
        let base_instructions = base_instructions.into();
        if base_instructions.trim().is_empty() {
            return Err(AgentProfileError::EmptyInstructions);
        }
        Ok(Self {
            id: AgentProfileId::new(id)?,
            base_instructions,
            documents: None,
            model_provider: None,
            workspace_instructions: WorkspaceInstructions::None,
            turn_timing: TurnTimingPolicy::None,
            automatic_compaction: None,
        })
    }

    pub(crate) fn with_documents(mut self, documents: ProfileDocuments) -> Self {
        self.documents = Some(documents);
        self
    }

    /// Restricts sessions assembled from this profile to one model provider.
    #[must_use]
    pub const fn with_model_provider(mut self, provider: ModelProvider) -> Self {
        self.model_provider = Some(provider);
        self
    }

    pub(crate) const fn model_provider(&self) -> Option<ModelProvider> {
        self.model_provider
    }

    /// Adds durable Host time and elapsed-user-message context to every turn.
    ///
    /// The Host keeps changing time outside the stable system prompt and
    /// restores the exact observation on retry.
    #[must_use]
    pub const fn with_turn_timing(mut self) -> Self {
        self.turn_timing = TurnTimingPolicy::HostClock;
        self
    }

    pub(crate) const fn uses_turn_timing(&self) -> bool {
        matches!(self.turn_timing, TurnTimingPolicy::HostClock)
    }

    /// Starts automatic context compaction at one exact model-input estimate
    /// and bounds the rebuilt request after compaction.
    #[must_use]
    pub const fn with_automatic_compaction(
        mut self,
        trigger_input_tokens: NonZeroU64,
        target_input_tokens: NonZeroU64,
    ) -> Self {
        self.automatic_compaction = Some(AutomaticCompactionPolicy {
            trigger_input_tokens,
            target_input_tokens,
        });
        self
    }

    pub(crate) const fn automatic_compaction(&self) -> Option<AutomaticCompactionPolicy> {
        self.automatic_compaction
    }

    /// Adds the canonical workspace-root `AGENTS.md` to this profile.
    #[must_use]
    pub const fn with_workspace_instructions(mut self) -> Self {
        self.workspace_instructions = WorkspaceInstructions::RootAgentsFile;
        self
    }

    #[must_use]
    pub const fn id(&self) -> &AgentProfileId {
        &self.id
    }

    pub(crate) fn document_binding(&self) -> Option<renoa_agent_loop::AgentToolBinding> {
        self.documents
            .as_ref()
            .map(|documents| documents.binding(self.id.clone()))
    }

    pub(crate) fn system_prompt(&self, workspace: &Path) -> Result<String, AgentProfileError> {
        let documents = self
            .documents
            .as_ref()
            .map(ProfileDocuments::render)
            .transpose()?;
        let project = match self.workspace_instructions {
            WorkspaceInstructions::None => None,
            WorkspaceInstructions::RootAgentsFile => project_instructions(workspace, &self.id)?,
        };
        if documents.is_none() && project.is_none() {
            return Ok(self.base_instructions.trim_end().to_owned());
        }
        let capacity = self.base_instructions.len()
            + documents.as_ref().map_or(0, String::len)
            + project.as_ref().map_or(0, String::len)
            + 192;
        let mut prompt = String::with_capacity(capacity);
        prompt.push_str(self.base_instructions.trim_end());
        if let Some(documents) = documents {
            prompt.push_str("\n\n");
            prompt.push_str(&documents);
        }
        if let Some(project) = project {
            let project = project.strip_prefix('\u{feff}').unwrap_or(&project);
            if !project.trim().is_empty() {
                prompt.push_str("\n\n<project_instructions source=\"AGENTS.md\">\n");
                prompt.push_str(project);
                if !project.ends_with('\n') {
                    prompt.push('\n');
                }
                prompt.push_str("</project_instructions>");
            }
        }
        Ok(prompt)
    }
}

/// Invalid agent-profile identity, instructions, or workspace rules.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum AgentProfileError {
    #[error(
        "agent profile id must be 1-{MAX_PROFILE_ID_BYTES} ASCII letters, digits, '_', '-', or '.'"
    )]
    InvalidId,
    #[error("agent profile base instructions must not be empty")]
    EmptyInstructions,
    #[error("cannot inspect project instructions for profile `{profile}` at `{path}`: {source}")]
    Inspect {
        profile: AgentProfileId,
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    #[error("project instructions for profile `{profile}` resolve outside the workspace: {path}")]
    OutsideWorkspace {
        profile: AgentProfileId,
        path: PathBuf,
    },
    #[error("project instructions for profile `{profile}` must be a regular file: {path}")]
    NotFile {
        profile: AgentProfileId,
        path: PathBuf,
    },
    #[error(
        "project instructions for profile `{profile}` at `{path}` exceed the {MAX_PROJECT_INSTRUCTIONS_BYTES}-byte limit"
    )]
    TooLarge {
        profile: AgentProfileId,
        path: PathBuf,
    },
    #[error("project instructions for profile `{profile}` at `{path}` are not UTF-8: {source}")]
    InvalidUtf8 {
        profile: AgentProfileId,
        path: PathBuf,
        #[source]
        source: std::string::FromUtf8Error,
    },
    #[error("cannot {operation} at `{path}`: {source}")]
    DocumentIo {
        operation: &'static str,
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    #[error("profile document path has no parent directory: {path}")]
    DocumentPath { path: PathBuf },
    #[error(
        "profile document directory for `{profile}` resolves outside the Host data directory: {path}"
    )]
    DocumentOutsideDataDirectory {
        profile: AgentProfileId,
        path: PathBuf,
    },
    #[error("profile document must be a regular file: {path}")]
    DocumentNotFile { path: PathBuf },
    #[error("profile document at `{path}` is not UTF-8: {source}")]
    DocumentInvalidUtf8 {
        path: PathBuf,
        #[source]
        source: std::string::FromUtf8Error,
    },
}

fn project_instructions(
    workspace: &Path,
    profile: &AgentProfileId,
) -> Result<Option<String>, AgentProfileError> {
    let candidate = workspace.join(PROJECT_INSTRUCTIONS_FILE);
    match fs::symlink_metadata(&candidate) {
        Ok(_) => {}
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(source) => {
            return Err(AgentProfileError::Inspect {
                profile: profile.clone(),
                path: candidate,
                source,
            });
        }
    }
    let resolved = fs::canonicalize(&candidate).map_err(|source| AgentProfileError::Inspect {
        profile: profile.clone(),
        path: candidate,
        source,
    })?;
    if !resolved.starts_with(workspace) {
        return Err(AgentProfileError::OutsideWorkspace {
            profile: profile.clone(),
            path: resolved,
        });
    }
    let metadata = fs::metadata(&resolved).map_err(|source| AgentProfileError::Inspect {
        profile: profile.clone(),
        path: resolved.clone(),
        source,
    })?;
    if !metadata.is_file() {
        return Err(AgentProfileError::NotFile {
            profile: profile.clone(),
            path: resolved,
        });
    }
    if metadata.len() > MAX_PROJECT_INSTRUCTIONS_BYTES_U64 {
        return Err(AgentProfileError::TooLarge {
            profile: profile.clone(),
            path: resolved,
        });
    }
    let file = File::open(&resolved).map_err(|source| AgentProfileError::Inspect {
        profile: profile.clone(),
        path: resolved.clone(),
        source,
    })?;
    let mut bytes = Vec::with_capacity(MAX_PROJECT_INSTRUCTIONS_BYTES);
    file.take(MAX_PROJECT_INSTRUCTIONS_BYTES_U64 + 1)
        .read_to_end(&mut bytes)
        .map_err(|source| AgentProfileError::Inspect {
            profile: profile.clone(),
            path: resolved.clone(),
            source,
        })?;
    if bytes.len() > MAX_PROJECT_INSTRUCTIONS_BYTES {
        return Err(AgentProfileError::TooLarge {
            profile: profile.clone(),
            path: resolved,
        });
    }
    String::from_utf8(bytes)
        .map(Some)
        .map_err(|source| AgentProfileError::InvalidUtf8 {
            profile: profile.clone(),
            path: resolved,
            source,
        })
}
