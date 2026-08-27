mod error;
mod package;
mod projector;
mod registry;
mod render;
mod store;
mod tool;

use std::{
    env,
    path::{Path, PathBuf},
    sync::Arc,
};

use renoa_agent_loop::ContextProjector;
use renoa_kernel::{CommandId, SessionId};

pub use error::SkillError;
pub(crate) use store::SkillStore;
pub(crate) use tool::alpha_skill_bindings;

pub(crate) struct SkillRuntimeContext {
    pub(crate) instructions: String,
    pub(crate) projector: Arc<dyn ContextProjector>,
    pub(crate) revision: String,
}

pub(crate) fn runtime_context(
    store: &SkillStore,
    session_id: SessionId,
    current_command_id: Option<CommandId>,
) -> Result<Option<SkillRuntimeContext>, SkillError> {
    let Some(active) = render::active(&store.active(session_id, current_command_id)?)? else {
        return Ok(None);
    };
    Ok(Some(SkillRuntimeContext {
        instructions: active.instructions,
        projector: Arc::new(projector::ActivatedSkillProjector::new(active.references)),
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
    data_directory.join("skills")
}
