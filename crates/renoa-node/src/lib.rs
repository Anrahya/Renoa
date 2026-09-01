//! Durable execution-node bridge between RCP and Renoa's local Host.

mod bridge;
mod node_store;
mod projection;
mod session;

pub use bridge::{HostTarget, NodeError, RenoaNode};
