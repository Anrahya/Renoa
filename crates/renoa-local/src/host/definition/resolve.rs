//! Runtime resolution of one canonical agent definition.
//!
//! Resolution reads the root row and its children, attaches the agent's own
//! document root, and composes the system prompt for one workspace. A preset is
//! never consulted here: the stored definition is the only operational owner.

use std::{fs, io::Read as _, path::Path};

use renoa_agent_loop::AgentToolBinding;
use renoa_kernel::AgentId;

use super::store;
use crate::{
    AgentBehavior, AgentDefinition, AgentDefinitionError, AgentDocuments, AgentToolSelection,
    AutomaticCompaction, ModelProvider,
    documents::AgentDocumentStore,
    host::{HostConfig, LocalHostError, catalog},
};

const PROJECT_INSTRUCTIONS_FILE: &str = "AGENTS.md";
const MAX_PROJECT_INSTRUCTIONS_BYTES: usize = 32 * 1024;
const MAX_PROJECT_INSTRUCTIONS_BYTES_U64: u64 = 32 * 1024;

/// One agent's definition plus the state only the runtime needs.
pub struct ResolvedAgentDefinition {
    definition: AgentDefinition,
    documents: Option<AgentDocumentStore>,
}

impl ResolvedAgentDefinition {
    #[must_use]
    pub(crate) fn agent_id(&self) -> AgentId {
        self.definition.id
    }

    #[must_use]
    pub(crate) fn behavior(&self) -> AgentBehavior {
        self.definition.operational.behavior
    }

    #[must_use]
    pub(crate) fn selected_tools(&self) -> &AgentToolSelection {
        &self.definition.tool_selection
    }

    #[must_use]
    pub(crate) fn connections(&self) -> &std::collections::BTreeSet<String> {
        &self.definition.connections
    }

    #[must_use]
    pub(crate) fn documents(&self) -> Option<AgentDocuments> {
        self.definition.operational.documents
    }

    #[must_use]
    pub(crate) fn provider_restriction(&self) -> Option<ModelProvider> {
        self.definition.operational.provider_restriction
    }

    #[must_use]
    pub(crate) fn automatic_compaction(&self) -> Option<AutomaticCompaction> {
        self.definition.operational.behavior.automatic_compaction
    }

    /// Composes the system prompt for one workspace.
    ///
    /// # Errors
    ///
    /// Returns an error when an enabled document or the workspace project
    /// instructions cannot be read safely.
    pub fn system_prompt(&self, workspace: &Path) -> Result<String, AgentDefinitionError> {
        let base = &self.definition.operational.instructions;
        let documents = self
            .documents
            .as_ref()
            .map(AgentDocumentStore::render)
            .transpose()?;
        let project = if self
            .definition
            .operational
            .behavior
            .loads_project_instructions()
        {
            project_instructions(workspace, self.definition.id)?
        } else {
            None
        };
        if documents.is_none() && project.is_none() {
            return Ok(base.trim_end().to_owned());
        }
        let capacity = base.len()
            + documents.as_ref().map_or(0, String::len)
            + project.as_ref().map_or(0, String::len)
            + 192;
        let mut prompt = String::with_capacity(capacity);
        prompt.push_str(base.trim_end());
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

    /// Builds the tool binding that edits this agent's own documents.
    #[must_use]
    pub(crate) fn document_binding(&self) -> Option<AgentToolBinding> {
        self.documents.as_ref().map(AgentDocumentStore::binding)
    }
}

/// Resolves one agent's definition for runtime composition.
///
/// # Errors
///
/// Returns an error for an unknown agent, corrupt stored state, or an
/// unreadable document root.
pub(crate) async fn resolve_definition(
    host: &HostConfig,
    agent: AgentId,
) -> Result<ResolvedAgentDefinition, LocalHostError> {
    let database = host.database.clone();
    let data_directory = host
        .sessions
        .parent()
        .map(Path::to_path_buf)
        .ok_or_else(|| {
            LocalHostError::InvalidRequest("Host sessions have no data root".to_owned())
        })?;
    tokio::task::spawn_blocking(move || {
        let connection = catalog::open_verified(&database)?;
        let definition =
            store::read(&connection, agent)?.ok_or(LocalHostError::AgentNotFound(agent))?;
        let documents = definition
            .operational
            .documents
            .map(|enabled| AgentDocumentStore::open(&data_directory, agent, enabled))
            .transpose()?;
        Ok(ResolvedAgentDefinition {
            definition,
            documents,
        })
    })
    .await?
}

fn project_instructions(
    workspace: &Path,
    agent: AgentId,
) -> Result<Option<String>, AgentDefinitionError> {
    let candidate = workspace.join(PROJECT_INSTRUCTIONS_FILE);
    match fs::symlink_metadata(&candidate) {
        Ok(_) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(source) => {
            return Err(AgentDefinitionError::Inspect {
                agent,
                path: candidate,
                source,
            });
        }
    }
    let resolved =
        fs::canonicalize(&candidate).map_err(|source| AgentDefinitionError::Inspect {
            agent,
            path: candidate.clone(),
            source,
        })?;
    if !resolved.starts_with(workspace) {
        return Err(AgentDefinitionError::OutsideWorkspace {
            agent,
            path: resolved,
        });
    }
    let metadata = fs::metadata(&resolved).map_err(|source| AgentDefinitionError::Inspect {
        agent,
        path: resolved.clone(),
        source,
    })?;
    if !metadata.is_file() {
        return Err(AgentDefinitionError::NotFile {
            agent,
            path: resolved,
        });
    }
    if metadata.len() > MAX_PROJECT_INSTRUCTIONS_BYTES_U64 {
        return Err(AgentDefinitionError::TooLarge {
            agent,
            path: resolved,
        });
    }
    let file = fs::File::open(&resolved).map_err(|source| AgentDefinitionError::Inspect {
        agent,
        path: resolved.clone(),
        source,
    })?;
    let mut bytes = Vec::with_capacity(MAX_PROJECT_INSTRUCTIONS_BYTES);
    file.take(MAX_PROJECT_INSTRUCTIONS_BYTES_U64 + 1)
        .read_to_end(&mut bytes)
        .map_err(|source| AgentDefinitionError::Inspect {
            agent,
            path: resolved.clone(),
            source,
        })?;
    if bytes.len() > MAX_PROJECT_INSTRUCTIONS_BYTES {
        return Err(AgentDefinitionError::TooLarge {
            agent,
            path: resolved,
        });
    }
    String::from_utf8(bytes)
        .map(Some)
        .map_err(|source| AgentDefinitionError::InvalidUtf8 {
            agent,
            path: resolved,
            source,
        })
}
