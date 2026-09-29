//! Durable execution-node bridge between RCP and Renoa's local Host.

mod agent_targets;
mod automation_cleanup;
mod automations;
mod backoff;
mod bridge;
mod live;
mod node_log;
mod node_store;
mod operator;
mod projection;
mod session;

pub use bridge::{NodeError, RenoaNode};
