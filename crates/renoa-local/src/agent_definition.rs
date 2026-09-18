use std::{
    collections::BTreeSet,
    fmt,
    io,
    num::NonZeroU64,
    path::PathBuf,
    str::FromStr,
};

use renoa_kernel::AgentId;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use thiserror::Error;
use uuid::Uuid;

use crate::ModelProvider;

const MAX_PRESET_ID_BYTES: usize = 128;
pub(crate) const MAX_NAME_BYTES: usize = 512;
pub(crate) const MAX_INSTRUCTIONS_BYTES: usize = 32 * 1024;
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
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum AgentCreator {
    /// Another agent created this agent through an agent-facing tool.
    Agent { agent_id: AgentId },
    /// An authenticated management principal created this agent.
    Principal { host_id: Uuid, principal_id: String },
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

    /// Returns the creating agent when an agent made the call.
    #[must_use]
    pub const fn agent_id(&self) -> Option<AgentId> {
        match self {
            Self::Agent { agent_id } => Some(*agent_id),
            Self::Principal { .. } | Self::System { .. } => None,
        }
    }
}

/// How the create call reached the Host. Record data only, never authority.
#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
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

/// Whether a turn carries durable Host time and elapsed-message context.
#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum TurnTiming {
    Off,
    HostClock,
}

/// Whether the workspace-root project instruction file joins the prompt.
#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum WorkspaceInstructions {
    Off,
    ProjectAgentsFile,
}

/// Exact model-input boundaries for automatic context compaction.
#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct AutomaticCompaction {
    pub trigger_input_tokens: NonZeroU64,
    pub target_input_tokens: NonZeroU64,
}

/// The operational behavior a Host runtime composes from one definition.
#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct AgentBehavior {
    pub turn_timing: TurnTiming,
    pub workspace_instructions: WorkspaceInstructions,
    pub automatic_compaction: Option<AutomaticCompaction>,
}

impl AgentBehavior {
    #[must_use]
    pub const fn uses_turn_timing(self) -> bool {
        matches!(self.turn_timing, TurnTiming::HostClock)
    }

    #[must_use]
    pub const fn loads_project_instructions(self) -> bool {
        matches!(
            self.workspace_instructions,
            WorkspaceInstructions::ProjectAgentsFile
        )
    }
}

/// Which owner-editable prompt documents this agent keeps.
#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct AgentDocuments {
    pub soul: bool,
    pub user: bool,
}

impl AgentDocuments {
    #[must_use]
    pub const fn any(self) -> bool {
        self.soul || self.user
    }
}

/// The core operational document. Every agent row stores exactly one.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct AgentOperationalDefinition {
    pub instructions: String,
    pub behavior: AgentBehavior,
    pub documents: Option<AgentDocuments>,
    pub provider_restriction: Option<ModelProvider>,
}

impl AgentOperationalDefinition {
    /// Rejects instructions a runtime could not use.
    ///
    /// # Errors
    ///
    /// Returns an error for blank or oversized instructions.
    pub fn validate(&self) -> Result<(), AgentDefinitionError> {
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
        Ok(())
    }
}

/// The exact, revisioned capability names one agent may invoke.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct AgentToolSelection {
    pub revision: i64,
    pub tools: BTreeSet<String>,
}

/// The canonical Host-owned definition of one durable agent.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct AgentDefinition {
    pub id: AgentId,
    pub name: String,
    pub created_at_ms: i64,
    pub creator: AgentCreator,
    pub created_via: AgentCreationOrigin,
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
        if self.name.trim().is_empty() || self.name.trim() != self.name {
            return Err(AgentDefinitionError::InvalidName);
        }
        if self.name.len() > MAX_NAME_BYTES {
            return Err(AgentDefinitionError::InvalidName);
        }
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
    #[error("agent name must contain 1-{MAX_NAME_BYTES} bytes without surrounding whitespace")]
    InvalidName,
    #[error("agent instructions must not be empty")]
    EmptyInstructions,
    #[error("agent instructions exceed {MAX_INSTRUCTIONS_BYTES} bytes: {bytes}")]
    InstructionsTooLarge { bytes: usize },
    #[error("agent creator must carry a non-empty actor identifier")]
    InvalidActor,
    #[error("agent preset `{preset}` is not registered with this Host")]
    UnknownPreset { preset: String },
    #[error("agent preset `{preset}` supplies its own instructions and rejects overrides")]
    InstructionsNotAllowed { preset: String },
    #[error("agent preset `{preset}` requires caller-supplied instructions")]
    InstructionsRequired { preset: String },
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
    #[error("cannot inspect project instructions at `{path}`: {source}")]
    Inspect {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    #[error("project instructions at `{path}` resolve outside the workspace")]
    OutsideWorkspace { path: PathBuf },
    #[error("project instructions must be a regular file: {path}")]
    NotFile { path: PathBuf },
    #[error("project instructions exceed {MAX_INSTRUCTIONS_BYTES} bytes: {path}")]
    TooLarge { path: PathBuf },
    #[error("project instructions at `{path}` are not UTF-8: {source}")]
    InvalidUtf8 {
        path: PathBuf,
        #[source]
        source: std::string::FromUtf8Error,
    },
}
