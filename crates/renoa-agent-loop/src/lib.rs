//! Provider-neutral model/tool behavior for the durable Renoa kernel.

mod adapters;
mod configuration;
mod context;
mod decision;
mod format;

pub use configuration::{
    AgentLoopBuildError, AgentLoopConfig, AgentToolBinding, ContextBinding, ModelBinding,
    build_runtime,
};
pub use context::{ContextInput, ContextStrategy, ContextStrategyError, FullHistoryStrategy};
pub use format::{AgentCommand, MESSAGE_EVENT_KIND};
