//! Durable execution-node bridge between RCP and Renoa's local Host.

mod agent_link;
mod agent_targets;
mod automation_cleanup;
mod automations;
mod backoff;
mod bridge;
mod control;
mod execution;
mod host_control;
mod live;
mod live_text;
mod node_log;
mod node_store;
mod origin;
mod owner_wakes;
mod projection;
mod session;
mod setup;
mod stop;

pub use bridge::{NodeError, RenoaNode};
