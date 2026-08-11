//! Harness-neutral values carried by the Renoa Continuity Protocol.

mod command;
mod execution;
mod ids;

pub use command::{CommandEnvelope, CommandInput, SurfaceRef, TargetRef};
pub use execution::{ExecutionEvent, ExecutionEventKind, ExecutionTerminal};
pub use ids::{CommandId, ExecutionEventId, ExecutionId, PrincipalId};
