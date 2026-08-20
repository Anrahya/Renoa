//! Provider-neutral model/tool behavior for the durable Renoa kernel.

mod adapters;
mod compaction;
mod configuration;
mod context;
mod decision;
mod format;

pub use compaction::{
    CompactingContextStrategy, CompactionCheckpoint, CompactionLimits, CompactionLimitsError,
    CompactionPlan, CompactionPlanner, CompactionPlanningError, ContextSizer,
};
pub use configuration::{
    AgentLoopBuildError, AgentLoopConfig, AgentToolBinding, ContextBinding, ModelBinding,
    build_runtime,
};
pub use context::{
    CompactionValidationError, ContextEntry, ContextInput, ContextPreparation, ContextStrategy,
    ContextStrategyError, FullHistoryStrategy,
};
pub use format::{AgentCommand, CONTEXT_CHECKPOINT_EVENT_KIND, MESSAGE_EVENT_KIND};
