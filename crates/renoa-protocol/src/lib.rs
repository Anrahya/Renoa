//! Harness-neutral values carried by the Renoa Continuity Protocol.

mod command;
mod execution;
mod ids;

pub use command::{
    AgentMessageKind, AgentOrigin, Author, CommandEnvelope, CommandInput, SurfaceRef, TargetCard,
    TargetRef,
};
pub use execution::{ExecutionEvent, ExecutionEventKind, ExecutionTerminal};
pub use ids::{CommandId, ExecutionEventId, ExecutionId, PrincipalId};
