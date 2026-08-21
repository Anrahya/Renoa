use std::{future::Future, pin::Pin};

mod agent;
mod context;
mod control;
mod events;
mod message;
mod model;
mod sampling;
mod state;
mod tool;

pub use agent::{Agent, AgentConfig, AgentConfigError, AgentError, AgentRunResult};
pub use context::{ContextProjectionError, ContextProjector};
pub use control::{AgentHandle, QueueError, QueueMode};
pub use events::{AgentEvent, AgentEventSink, ModelFailureCode};
pub use message::{AssistantContent, AssistantMetadata, ContentBlock, Message, MessageRole};
pub use model::{
    AssistantDelta, Model, ModelError, ModelErrorKind, ModelEvent, ModelEventStream, ModelRequest,
    ModelResponse, StopReason, TokenUsage,
};
pub use sampling::{SamplingError, SamplingResult, sample_model};
pub use state::AgentState;
pub use tool::{
    Tool, ToolCall, ToolCallBatchError, ToolError, ToolErrorCode, ToolExecutionMode,
    ToolOutcomeUnknown, ToolOutput, ToolResult, ToolSpec, ToolUpdates, invoke_tool,
    validate_tool_call_ids,
};

pub type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;
