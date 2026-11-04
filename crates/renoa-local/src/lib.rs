//! First local Host for composable Renoa agent runtimes.

mod agent_definition;
mod agent_model;
mod agent_session;
mod agent_trace;
mod atomic_file;
mod bash;
mod capabilities;
mod code_mode;
mod compact_invocation;
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
mod speaker;
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
    AutomaticCompaction, WorkspaceInstructions,
};
pub use agent_model::AgentModelSelection;
pub use agent_session::{AgentSession, AgentSessionConfiguration};
pub use code_mode::validate_code_mode_worker;
pub use compact_invocation::{CompactInvocation, compact_invocation};
pub use documents::UserProfile;
pub use host::catalog::HostCatalogError;
pub use host::definition::{
    AgentApp, AgentAutomation, AgentCreateRequest, AgentDefinitionPage, AgentToolsUpdate,
    HostMainAgentControl, MAX_AGENT_PAGE, MainAgent, MainAgentChange, MainAgentError, PresetMove,
    RenameAgent, ResolvedAgentDefinition, derived_agent_id,
};
pub use host::history::AgentSessionHistory;
pub use host::observation::{
    HostObservation, HostObserver, ObservedAgent, ObservedAutomation, ObservedConnection,
    ObservedMemoryPasses, ObservedOperation, ObservedOperationState, ObservedPlugin,
    ObservedRegistryFailure, ObservedRun, ObservedScheduler, ObservedSession, ObservedSessionState,
    ObservedSharedRegistry, ObservedSkill,
};
pub use host::{
    AgentPeers, HostResetReport, LocalHost, LocalHostAdapters, LocalHostError,
    LocalModelConfiguration, PeerAgent, PeerError, PeerMessage,
    profiles::{MemoryNotes, ProfileEditError},
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
    HostPluginActivation, InvalidSetupLink, MAX_PLUGIN_PAGE, PLUGIN_API_REVISION,
    PluginAuthentication, PluginAuthorizationRequired, PluginCredentialKind,
    PluginCredentialRequired, PluginHeader, PluginInventoryItem, PluginInventoryPage,
    PluginInvocation, PluginOutcome, PluginProgress, PluginRequest, PluginSource, SetupLink,
    SetupLinkKind, plugin_api_schema,
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
pub use speaker::Speaker;
pub use turn_observation::{TurnObservation, TurnObservationError};
pub use workspace::{LocalWorkspace, LocalWorkspaceError};

pub use host::automations::{
    AutomationEnablement, AutomationError, AutomationMutation, AutomationRecord,
    AutomationResultSummary, AutomationRun, AutomationRunRequest, AutomationSchedule,
    AutomationScheduler, AutomationSpec, HostAutomationControl, PluginRun, RunLanding, RunOutcome,
    RunResult, RunStatus, ScheduledRun,
};
pub use host::feed::{
    FEED_PAGE, FeedAction, FeedAgent, FeedError, FeedItem, FeedKind, FeedPage, FeedSeen, FeedWake,
    HostFeed, Urgency,
};

pub use renoa_home::RenoaHome;
