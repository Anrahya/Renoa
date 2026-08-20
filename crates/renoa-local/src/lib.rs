//! First local Host for composable Renoa agent runtimes.

mod alpha;
mod bash;
mod file_tools;
mod output;
mod pi_catalog;
mod pi_context;
mod pi_model;
mod pi_stream;
mod process;
mod ripgrep;
mod runtime;
mod search;
mod tool_input;
mod workspace;

pub use alpha::AlphaError;
pub use pi_catalog::{PiModelOption, PiReasoningLevel, discover_pi_models};
pub use pi_model::{PiModel, PiModelConfigError};
pub use runtime::{
    LocalRuntimeConfig, LocalRuntimeError, build_local_profile, build_local_runtime,
};
pub use workspace::{LocalWorkspace, LocalWorkspaceError};
