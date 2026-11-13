//! Harness-neutral values carried by the Renoa Continuity Protocol.

mod attachment;
mod command;
mod execution;
mod ids;
mod live;

pub use attachment::Attachment;
pub use command::{
    AgentMessageKind, AgentOrigin, Author, CommandEnvelope, CommandInput, SurfaceRef, TargetCard,
    TargetRef,
};
pub use execution::{ExecutionEvent, ExecutionEventKind, ExecutionTerminal};
pub use ids::{CommandId, ExecutionEventId, ExecutionId, PrincipalId};
pub use live::{LiveActivity, MAX_ACTIVITY_BYTES, normalize_activity, strip_format_chars};
