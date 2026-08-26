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
    build_runtime, build_runtime_with_events,
};
pub use context::{
    CompactionValidationError, ContextEntry, ContextInput, ContextPreparation, ContextProjector,
    ContextStrategy, ContextStrategyError, ExplicitCompactionPreparation, FullHistoryStrategy,
};
pub use format::{
    AgentCommand, COMPACTION_RESULT_EVENT_KIND, CONTEXT_CHECKPOINT_EVENT_KIND, CompactionResult,
    MESSAGE_EVENT_KIND,
};
