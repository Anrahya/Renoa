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
mod mcp;
mod model_bridge;
mod model_catalog;
mod model_context;
mod model_stream;
mod output;
mod process;
mod ripgrep;
mod runtime;
mod search;
mod selection;
mod session;
mod skills;
mod tool_error;
mod tool_input;
mod trace;
mod workspace;

#[cfg(test)]
mod model_adapter_process_tests;

pub use alpha::{ALPHA_PROFILE_ID, AlphaError};
pub use alpha_session::{AlphaSession, AlphaSessionConfiguration};
pub use host::catalog::HostCatalogError;
pub use host::{LocalHost, LocalHostError};
pub use mcp::{
    AlphaMcpTool, McpAdapterError, McpCatalogSnapshot, McpCatalogTool, McpCredentialError,
    McpFailureKind, McpHostError, McpOutcomeCertainty, McpRejectedTool, McpRemoteFailure,
};
pub use model_bridge::{BridgeModel, ModelBridgeError};
pub use model_catalog::{ModelChoice, ModelProvider, ReasoningLevel, discover_models};
pub use runtime::{
    LocalRuntimeConfig, LocalRuntimeError, build_local_runtime, build_local_runtime_with_events,
};
pub use session::{LocalHistoryEntry, LocalSession, LocalSessionError, LocalTurnOutcome};
pub use skills::SkillError;
pub use workspace::{LocalWorkspace, LocalWorkspaceError};
