mod error;
pub(crate) mod package;
mod projector;
mod registry;
mod render;
pub(crate) mod store;
mod tool;

use std::{
    env,
    path::{Path, PathBuf},
    sync::Arc,
};

use renoa_agent_loop::ContextProjector;
use renoa_kernel::{CommandId, SessionId};

pub use error::SkillError;
pub(crate) use store::{SkillComponentReport, SkillSourceReport, SkillStore};
pub(crate) use tool::agent_skill_bindings;

pub(crate) struct SkillRuntimeContext {
    pub(crate) instructions: String,
    pub(crate) projector: Arc<dyn ContextProjector>,
    pub(crate) revision: String,
}

/// Activates one named skill through the shared content-addressed catalog and
/// renders it, or returns an empty string when the agent cannot see it.
#[cfg(test)]
pub(crate) fn frozen_instructions(
    store: &SkillStore,
    profile: &str,
    workspace: &Path,
    session_id: SessionId,
    command_id: CommandId,
    name: &str,
) -> Result<String, SkillError> {
    store.sync(profile, workspace)?;
    match store.activate(profile, workspace, session_id, command_id, name) {
        Ok(skill) => render::one(&skill),
        Err(SkillError::NotFound(_)) => Ok(String::new()),
        Err(error) => Err(error),
    }
}

pub(crate) fn runtime_context(
    store: &SkillStore,
    session_id: SessionId,
    current_command_id: Option<CommandId>,
    embedded_instructions: &str,
) -> Result<Option<SkillRuntimeContext>, SkillError> {
    let Some(active) = render::active(
        &store.active(session_id, current_command_id)?,
        embedded_instructions,
    )?
    else {
        return Ok(None);
    };
    Ok(Some(SkillRuntimeContext {
        instructions: active.instructions,
        projector: Arc::new(projector::ActivatedSkillProjector::new(active.bodies)),
        revision: active.revision,
    }))
}

pub(crate) fn default_global_source() -> Option<PathBuf> {
    home_directory().map(|home| home.join(".agents/skills"))
}

fn home_directory() -> Option<PathBuf> {
    #[cfg(windows)]
    let value = env::var_os("USERPROFILE");
    #[cfg(not(windows))]
    let value = env::var_os("HOME");
    value.filter(|value| !value.is_empty()).map(PathBuf::from)
}

pub(crate) fn store_path(data_directory: &Path) -> PathBuf {
    data_directory.join("state/skills")
}
