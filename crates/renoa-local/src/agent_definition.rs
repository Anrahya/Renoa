use std::{collections::BTreeSet, fmt, io, num::NonZeroU64, path::PathBuf, str::FromStr};

use renoa_kernel::AgentId;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use thiserror::Error;
use uuid::Uuid;

use crate::ModelProvider;

const MAX_PRESET_ID_BYTES: usize = 128;
pub(crate) const MAX_NAME_BYTES: usize = 512;
pub(crate) const MAX_INSTRUCTIONS_BYTES: usize = 32 * 1024;
pub(crate) const MAX_PURPOSE_BYTES: usize = 1024;
/// Derived from `renoa.app.v7`'s compaction target minus max summary tokens,
/// less the fixed prompt blocks, 19 reference tool schemas (`~24 KiB` + framing),
/// and 1,000 tokens reserved for `USER.md` and retained conversation tail.
pub(crate) const MAX_JOB_BYTES: usize = derived_max_job_bytes();

#[expect(
    clippy::cast_possible_truncation,
    reason = "compaction budget is ~90 KiB, far below u32::MAX"
)]
const fn derived_max_job_bytes() -> usize {
    let target = crate::presets::APP_COMPACTION_TARGET;
    let summary = if crate::runtime::MAX_CHECKPOINT_TOKENS < target / 4 {
        crate::runtime::MAX_CHECKPOINT_TOKENS
    } else {
        target / 4
    };
    let tail_budget_bytes =
        (target - summary) as usize * crate::model_context::ESTIMATED_BYTES_PER_TOKEN as usize;
    let fixed_bytes = crate::model_context::REQUEST_FRAME_TOKENS as usize
        * crate::model_context::ESTIMATED_BYTES_PER_TOKEN as usize
        + crate::presets::APP_INSTRUCTIONS.len()
        + crate::presets::APP_SOUL.len()
        + MAX_NAME_BYTES
        + MAX_PURPOSE_BYTES
        + crate::plugins::host::memory::SESSION_BLOCK_MAX
        + 384 // <identity>, <purpose>, <job>, <soul>, and <session_context> tags + revision hashes
        + 19 * (crate::model_context::TOOL_FRAME_TOKENS as usize
            * crate::model_context::ESTIMATED_BYTES_PER_TOKEN as usize
            + 16
            + 1_228
            + 18)
        + 3_000; // 1,000-token reserve for USER.md and retained post-compaction turns
    tail_budget_bytes - fixed_bytes
}
pub(crate) const MAX_CONNECTIONS: usize = 64;
pub(crate) const MAX_CONNECTION_ID_BYTES: usize = 256;
const MAX_ACTOR_ID_BYTES: usize = 256;

/// Stable identity of one code-owned creation preset.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct AgentPresetId(String);

impl AgentPresetId {
    /// Validates a preset identity before it reaches Host storage.
    ///
    /// # Errors
    ///
    /// Returns an error for an empty, oversized, or non-portable identifier.
    pub fn new(value: impl Into<String>) -> Result<Self, AgentDefinitionError> {
        let value = value.into();
        if value.is_empty()
            || value.len() > MAX_PRESET_ID_BYTES
            || !value
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-' | b'.'))
        {
            return Err(AgentDefinitionError::InvalidPresetId);
        }
        Ok(Self(value))
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for AgentPresetId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

impl FromStr for AgentPresetId {
    type Err = AgentDefinitionError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        Self::new(value)
    }
}

impl Serialize for AgentPresetId {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(self.as_str())
    }
}

impl<'de> Deserialize<'de> for AgentPresetId {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        Self::new(value).map_err(serde::de::Error::custom)
    }
}

/// The trusted, immutable record of who created one agent.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq, schemars::JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum AgentCreator {
    /// Another agent created this agent through an agent-facing tool.
    Agent {
        #[schemars(with = "String")]
        agent_id: AgentId,
    },
    /// An authenticated management principal created this agent.
    Principal {
        #[schemars(with = "String")]
        host_id: Uuid,
        principal_id: String,
    },
    /// Trusted local provisioning created this agent.
    System { component: String },
}

impl AgentCreator {
    pub(crate) fn validate(&self) -> Result<(), AgentDefinitionError> {
        match self {
            Self::Agent { .. } => Ok(()),
            Self::Principal { principal_id, .. } => require_actor_id(principal_id),
            Self::System { component } => require_actor_id(component),
        }
    }
}

/// How the create call reached the Host. Record data only, never authority.
#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum AgentCreationOrigin {
    AgentTool,
    Management,
    Provisioning,
}

impl AgentCreationOrigin {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::AgentTool => "agent_tool",
            Self::Management => "management",
            Self::Provisioning => "provisioning",
        }
    }
}

/// Whether the workspace-root project instruction file joins the prompt.
#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum WorkspaceInstructions {
    Off,
    ProjectAgentsFile,
}

/// Exact model-input boundaries for automatic context compaction.
#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct AutomaticCompaction {
    pub trigger_input_tokens: NonZeroU64,
    pub target_input_tokens: NonZeroU64,
}

/// The operational behavior a Host runtime composes from one definition.
#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct AgentBehavior {
    pub workspace_instructions: WorkspaceInstructions,
    pub automatic_compaction: Option<AutomaticCompaction>,
}

impl AgentBehavior {
    #[must_use]
    pub const fn loads_project_instructions(self) -> bool {
        matches!(
            self.workspace_instructions,
            WorkspaceInstructions::ProjectAgentsFile
        )
    }
}

/// Which prompt documents this agent reads: its own `SOUL.md`, and the
/// `USER.md` of the person each turn is talking to.
#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct AgentDocuments {
    /// Read the agent's own SOUL.md: its identity, judgment, and voice.
    pub soul: bool,
    /// Read the USER.md of whoever talks to the agent, the profile every agent
    /// that talks to that person shares. False for a job that does not need to
    /// know the person, such as a scheduled report.
    pub user: bool,
}

impl AgentDocuments {
    #[must_use]
    pub const fn any(self) -> bool {
        self.soul || self.user
    }
}

/// The core operational document. Every agent row stores exactly one.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct AgentOperationalDefinition {
    pub instructions: String,
    /// What the agent is for, in the words of whoever made it. The agent reads
    /// it in its prompt; an app shows it instead of `instructions`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub purpose: Option<String>,
    /// Job rules layered after `<purpose>` without replacing `instructions`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub job: Option<String>,
    pub behavior: AgentBehavior,
    pub documents: Option<AgentDocuments>,
    pub provider_restriction: Option<ModelProvider>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<crate::AgentModelSelection>,
}

impl AgentOperationalDefinition {
    /// Rejects instructions a runtime could not use.
    ///
    /// # Errors
    ///
    /// Returns an error for blank or oversized instructions.
    pub fn validate(&self) -> Result<(), AgentDefinitionError> {
        if let Some(model) = &self.model {
            model.validate()?;
        }
        if self
            .behavior
            .automatic_compaction
            .is_some_and(|policy| policy.target_input_tokens >= policy.trigger_input_tokens)
        {
            return Err(AgentDefinitionError::InvalidCompaction);
        }
        if self.instructions.trim().is_empty() {
            return Err(AgentDefinitionError::EmptyInstructions);
        }
        if self.instructions.len() > MAX_INSTRUCTIONS_BYTES {
            return Err(AgentDefinitionError::InstructionsTooLarge {
                bytes: self.instructions.len(),
            });
        }
        if self.documents.is_some_and(|documents| !documents.any()) {
            return Err(AgentDefinitionError::EmptyDocumentSet);
        }
        if let Some(purpose) = &self.purpose
            && !is_plain_text(purpose, MAX_PURPOSE_BYTES)
        {
            return Err(AgentDefinitionError::InvalidPurpose {
                bytes: purpose.len(),
            });
        }
        if let Some(job) = &self.job
            && (!is_plain_text(job, MAX_JOB_BYTES) || job.to_ascii_lowercase().contains("</job"))
        {
            return Err(AgentDefinitionError::InvalidJob { bytes: job.len() });
        }
        Ok(())
    }
}

/// A name is one line of plain text the system prompt can quote: not blank, at
/// most [`MAX_NAME_BYTES`], no surrounding whitespace, and no control character
/// or angle bracket that could end or open a prompt block.
///
/// # Errors
///
/// Returns [`AgentDefinitionError::InvalidName`] for any other name.
pub(crate) fn check_name(name: &str) -> Result<(), AgentDefinitionError> {
    let invalid = name.trim().is_empty()
        || name.trim() != name
        || name.len() > MAX_NAME_BYTES
        || name.chars().any(|c| c.is_control() || c == '<' || c == '>');
    if invalid {
        return Err(AgentDefinitionError::InvalidName);
    }
    Ok(())
}

/// Plain text bounded by `max_bytes`, not blank, with no control character other
/// than a newline.
fn is_plain_text(value: &str, max_bytes: usize) -> bool {
    !value.trim().is_empty()
        && value.len() <= max_bytes
        && !value.chars().any(|c| c.is_control() && c != '\n')
}

/// The exact, revisioned capability names one agent may invoke.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct AgentToolSelection {
    pub revision: i64,
    pub tools: BTreeSet<String>,
}

/// The canonical Host-owned definition of one durable agent.
// Its JSON Schema, generated from this type, is published at
// `docs/schemas/agent-definition.schema.json` (see `agent_definition_schema`).
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct AgentDefinition {
    #[schemars(with = "String")]
    pub id: AgentId,
    pub name: String,
    pub created_at_ms: i64,
    pub creator: AgentCreator,
    pub created_via: AgentCreationOrigin,
    #[schemars(with = "Option<String>")]
    pub preset_id: Option<AgentPresetId>,
    pub operational: AgentOperationalDefinition,
    pub tool_selection: AgentToolSelection,
    pub connections: BTreeSet<String>,
}

impl AgentDefinition {
    /// Validates every bounded field identically for every caller.
    ///
    /// # Errors
    ///
    /// Returns an error for an invalid name, instructions, creator, or
    /// connection set. Tool names are validated against the Host capability
    /// inventory by the caller that owns that inventory.
    pub fn validate(&self) -> Result<(), AgentDefinitionError> {
        check_name(&self.name)?;
        self.operational.validate()?;
        self.creator.validate()?;
        if self.tool_selection.revision <= 0 {
            return Err(AgentDefinitionError::InvalidToolSelectionRevision);
        }
        if self.connections.len() > MAX_CONNECTIONS {
            return Err(AgentDefinitionError::TooManyConnections {
                count: self.connections.len(),
            });
        }
        if self
            .connections
            .iter()
            .any(|id| id.is_empty() || id.len() > MAX_CONNECTION_ID_BYTES)
        {
            return Err(AgentDefinitionError::InvalidConnectionId);
        }
        Ok(())
    }
}

fn require_actor_id(value: &str) -> Result<(), AgentDefinitionError> {
    if value.trim().is_empty() || value.len() > MAX_ACTOR_ID_BYTES {
        return Err(AgentDefinitionError::InvalidActor);
    }
    Ok(())
}

/// Invalid agent-definition identity, instructions, or bound.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum AgentDefinitionError {
    #[error(
        "agent preset id must be 1-{MAX_PRESET_ID_BYTES} ASCII letters, digits, '_', '-', or '.'"
    )]
    InvalidPresetId,
    #[error(
        "agent name must be 1-{MAX_NAME_BYTES} bytes on one line, without surrounding whitespace, control characters, '<' or '>'. Send a short plain name such as News"
    )]
    InvalidName,
    #[error("agent instructions must not be empty")]
    EmptyInstructions,
    #[error("agent instructions exceed {MAX_INSTRUCTIONS_BYTES} bytes: {bytes}")]
    InstructionsTooLarge { bytes: usize },
    #[error(
        "agent purpose must be 1-{MAX_PURPOSE_BYTES} bytes of text that is not blank, with no control character but newline; received {bytes} bytes. Send it as a short plain sentence, or leave it out"
    )]
    InvalidPurpose { bytes: usize },
    #[error(
        "agent job must be 1-{MAX_JOB_BYTES} bytes of text that is not blank, with no `</job` closing tag and no control character but newline; received {bytes} bytes. Send it as plain text, or leave it out"
    )]
    InvalidJob { bytes: usize },
    #[error("agent creator must carry a non-empty actor identifier")]
    InvalidActor,
    #[error("agent model must contain 1-512 bytes without control characters")]
    InvalidModel,
    #[error("compaction target must be smaller than its trigger")]
    InvalidCompaction,
    #[error("agent preset `{preset}` is not registered with this Host")]
    UnknownPreset { preset: String },
    #[error("agent tool selection revision must be positive")]
    InvalidToolSelectionRevision,
    #[error("agent has more than {MAX_CONNECTIONS} connections: {count}")]
    TooManyConnections { count: usize },
    #[error("agent connection id must be 1-{MAX_CONNECTION_ID_BYTES} bytes")]
    InvalidConnectionId,
    #[error("agent documents for `{agent}` resolve outside the Host data directory: {path}")]
    DocumentsOutsideDataDirectory { agent: AgentId, path: PathBuf },
    #[error("an agent document set must enable at least one document")]
    EmptyDocumentSet,
    #[error("agent document at `{path}` already exists with different content")]
    DocumentConflict { path: PathBuf },
    #[error("agent document path has no parent directory: {path}")]
    DocumentPath { path: PathBuf },
    #[error("agent document must be a regular file: {path}")]
    DocumentNotFile { path: PathBuf },
    #[error("a user profile directory must be a plain directory, never a link: {path}")]
    ProfileDirectory { path: PathBuf },
    #[error("agent document publication at `{path}` was replaced before it settled")]
    DocumentPublicationReplaced { path: PathBuf },
    #[error("agent document at `{path}` is not UTF-8: {source}")]
    DocumentInvalidUtf8 {
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
    #[error(
        "agent document publication failed: {failure}; removing the partial publication at `{path}` also failed: {cleanup}"
    )]
    PublicationCleanup {
        path: PathBuf,
        failure: String,
        cleanup: String,
    },
    #[error("cannot inspect project instructions for `{agent}` at `{path}`: {source}")]
    Inspect {
        agent: AgentId,
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    #[error("project instructions for `{agent}` resolve outside the workspace: {path}")]
    OutsideWorkspace { agent: AgentId, path: PathBuf },
    #[error("project instructions for `{agent}` must be a regular file: {path}")]
    NotFile { agent: AgentId, path: PathBuf },
    #[error(
        "project instructions for `{agent}` at `{path}` exceed the {MAX_INSTRUCTIONS_BYTES}-byte limit"
    )]
    TooLarge { agent: AgentId, path: PathBuf },
    #[error("project instructions for `{agent}` at `{path}` are not UTF-8: {source}")]
    InvalidUtf8 {
        agent: AgentId,
        path: PathBuf,
        #[source]
        source: std::string::FromUtf8Error,
    },
}

#[cfg(test)]
mod tests;
