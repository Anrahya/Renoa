//! Durable execution-node bridge between RCP and Renoa's local Host.

mod agent_targets;
mod automation_cleanup;
mod automations;
mod backoff;
mod bridge;
mod control;
mod host_control;
mod live;
mod live_text;
mod node_log;
mod node_store;
mod operator;
mod projection;
mod session;
mod stop;

pub use bridge::{NodeError, RenoaNode};
