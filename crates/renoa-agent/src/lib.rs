use std::{future::Future, pin::Pin};

mod activity;
mod events;
mod message;
mod model;
mod sampling;
mod tool;

pub use activity::{
    ACTIVITY_FIELD, MAX_ACTIVITY_BYTES, TOOL_ACTIVITY_DESCRIPTION, extract_tool_activity,
    normalize_activity, schema_accepts_synthetic_activity, schema_uses_synthetic_activity,
    strip_activity_field, strip_tool_activity,
};
pub use events::{AgentEvent, AgentEventSink, ModelFailureCode};
pub use message::{AssistantContent, AssistantMetadata, ContentBlock, Message, MessageRole};
pub use model::{
    AssistantDelta, InferenceOutcome, Model, ModelError, ModelErrorKind, ModelEvent,
    ModelEventStream, ModelFailureDiagnostic, ModelRequest, ModelResponse, StopReason, TokenUsage,
};
pub use sampling::{SamplingError, SamplingResult, sample_model};
pub use tool::{
    Closing, Tool, ToolCall, ToolCallBatchError, ToolError, ToolErrorCode, ToolOutcomeUnknown,
    ToolOutput, ToolResult, ToolSpec, ToolUpdates, invoke_tool, validate_tool_call_ids,
};

pub type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;
