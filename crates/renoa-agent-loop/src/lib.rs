//! Provider-neutral model/tool behavior for the durable Renoa kernel.

mod adapters;
mod code_mode;
mod compaction;
mod configuration;
mod context;
mod decision;
mod format;
mod pending_tools;
mod turn_timing;

pub use code_mode::{
    CodeMcpCall, CodeStep, CodeStepOutput, CodeStepRequest, MAX_CODE_CALLS_PER_WAVE,
    MAX_CODE_MCP_ARGUMENT_BYTES, MAX_CODE_MCP_REFERENCE_BYTES, MAX_CODE_RESULT_BYTES,
    MAX_CODE_SNAPSHOT_BYTES,
};
pub use compaction::{
    CompactingContextStrategy, CompactionCheckpoint, CompactionLimits, CompactionLimitsError,
    CompactionPlan, CompactionPlanner, CompactionPlanningError, ContextSizer,
};
pub use configuration::{
    AgentLoopBuildError, AgentLoopConfig, AgentToolBinding, CodeModeBinding, ContextBinding,
    ModelBinding, build_runtime, build_runtime_with_code_mode,
    build_runtime_with_code_mode_and_events, build_runtime_with_events,
};
pub use context::{
    CompactionValidationError, ContextEntry, ContextInput, ContextPreparation, ContextProjector,
    ContextStrategy, ContextStrategyError, ExplicitCompactionPreparation, FullHistoryStrategy,
};
pub use format::{
    AgentCommand, COMPACTION_RESULT_EVENT_KIND, CONTEXT_CHECKPOINT_EVENT_KIND, CompactionResult,
    MESSAGE_EVENT_KIND, TURN_TIMING_EVENT_KIND,
};
pub use turn_timing::{TurnTiming, TurnTimingError};
mod usage;
pub use usage::recorded_token_usage;
