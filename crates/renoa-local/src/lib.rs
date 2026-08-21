//! First local Host for composable Renoa agent runtimes.

mod alpha;
mod alpha_session;
mod alpha_trace;
mod atomic_file;
mod bash;
mod deadline;
mod file_tools;
mod host;
mod host_storage;
mod output;
mod pi_catalog;
mod pi_context;
mod pi_model;
mod pi_stream;
mod process;
mod ripgrep;
mod runtime;
mod search;
mod selection;
mod session;
mod tool_error;
mod tool_input;
mod trace;
mod workspace;

pub use alpha::{ALPHA_PROFILE_ID, AlphaError};
pub use alpha_session::{AlphaSession, AlphaSessionConfiguration};
pub use host::{LocalHost, LocalHostError};
pub use pi_catalog::{PiModelOption, PiReasoningLevel, discover_pi_models};
pub use pi_model::{PiModel, PiModelConfigError};
pub use runtime::{
    LocalRuntimeConfig, LocalRuntimeError, build_local_runtime, build_local_runtime_with_events,
};
pub use session::{LocalHistoryEntry, LocalSession, LocalSessionError, LocalTurnOutcome};
pub use workspace::{LocalWorkspace, LocalWorkspaceError};
