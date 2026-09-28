//! First local Host for composable Renoa agent runtimes.

mod agent_definition;
mod agent_model;
mod agent_session;
mod agent_trace;
mod atomic_file;
mod bash;
mod capabilities;
mod code_mode;
mod credential_file;
mod deadline;
mod documents;
mod file_lock;
mod file_tools;
mod git_repository;
mod host;
mod host_storage;
mod mcp;
mod model_bridge;
mod model_catalog;
mod model_context;
mod model_stream;
mod output;
mod package_tree;
mod plugins;
mod presets;
mod process;
mod ripgrep;
mod runtime;
mod search;
mod selection;
mod session;
mod shared_registry;
mod skills;
mod stable_id;
mod tool_error;
mod tool_input;
mod trace;
mod turn_observation;
mod workspace;

#[cfg(test)]
mod model_adapter_process_tests;
#[cfg(test)]
mod test_agents;

pub use agent_definition::{
    AgentBehavior, AgentCreationOrigin, AgentCreator, AgentDefinition, AgentDefinitionError,
    AgentDocuments, AgentOperationalDefinition, AgentPresetId, AgentToolSelection,
    AutomaticCompaction, TurnTiming, WorkspaceInstructions,
};
pub use agent_model::AgentModelSelection;
pub use agent_session::{AgentSession, AgentSessionConfiguration};
pub use code_mode::validate_code_mode_worker;
pub use credential_file::credential_file_is_private;
pub use host::catalog::HostCatalogError;
pub use host::definition::{
    AgentCreateRequest, AgentDefinitionPage, AgentRoutine, AgentToolsUpdate, MAX_AGENT_PAGE,
    RenameAgent, ResolvedAgentDefinition, derived_agent_id,
};
pub use host::history::AgentSessionHistory;
pub use host::observation::{
    HostObservation, HostObserver, ObservedAgent, ObservedConnection, ObservedOperation,
    ObservedOperationState, ObservedPlugin, ObservedRoutine, ObservedSession, ObservedSessionState,
    ObservedSkill,
};
pub use host::{
    HostResetReport, LocalHost, LocalHostAdapters, LocalHostError, LocalModelConfiguration,
    reset_host_data_root,
};
pub use mcp::{
    McpAdapterError, McpCatalogSnapshot, McpCatalogTool, McpConnectionStatus, McpCredentialError,
    McpFailureKind, McpHostError, McpOutcomeCertainty, McpRejectedTool, McpRemoteFailure,
    ResolvedMcpTool,
};
pub use model_bridge::{BridgeModel, ModelBridgeError};
pub use model_catalog::{ModelChoice, ModelProvider, ReasoningLevel, discover_models};
pub use plugins::api::{
    HostPluginActivation, MAX_PLUGIN_PAGE, PLUGIN_API_REVISION, PluginAuthentication,
    PluginAuthorizationRequired, PluginCredentialKind, PluginCredentialRequired, PluginHeader,
    PluginInventoryItem, PluginInventoryPage, PluginInvocation, PluginOutcome, PluginProgress,
    PluginRequest, PluginSource, plugin_api_schema,
};
pub use plugins::{
    InstalledPlugin, PluginActivation, PluginAddOutcome, PluginConnectionOutcome, PluginCredential,
    PluginError, PluginInspection, PluginMcpServer, PluginMetadata, PluginNotice,
    PluginOAuthRegistration, PluginProviderFamily, PluginSourceReceipt,
};
pub use renoa_kernel::AgentId;
pub use runtime::{
    LocalRuntimeConfig, LocalRuntimeError, build_local_runtime, build_local_runtime_with_events,
};
pub use session::{LocalHistoryEntry, LocalSession, LocalSessionError, LocalTurnOutcome};
pub use shared_registry::SharedPluginSyncReport;
pub use skills::SkillError;
pub use skills::store::{SkillComponentRejection, SkillComponentReport};
pub use turn_observation::{TurnObservation, TurnObservationError};
pub use workspace::{LocalWorkspace, LocalWorkspaceError};

pub use host::routines::{
    HostRoutineControl, RoutineEnablement, RoutineError, RoutineMutation, RoutineRecord,
    RoutineResultSummary, RoutineRun, RoutineSchedule, RoutineSpec,
};

pub use renoa_home::RenoaHome;
