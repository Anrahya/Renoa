//! Cross-surface task continuity outside Renoa's execution kernel.

mod browser_identity;
mod browser_identity_http;
mod browser_identity_store;
mod browser_ticket_store;
mod connection;
mod control_migrations;
mod control_schema;
mod coordinator;
mod dispatch_store;
mod event_store;
mod identity;
mod identity_store;
mod ids;
mod json_ws;
mod node_messages;
mod operations;
mod store;
mod wire;

pub use coordinator::{ControlError, Coordinator, TaskSpec};
pub use identity::{
    ConnectionTicket, DeviceCredential, DeviceCredentials, EnrollmentToken, PasskeyBootstrapToken,
};
pub use ids::{DeviceId, NodeId, TaskEventId, TaskId};
pub use json_ws::{ClientMessage, JSON_WS_VERSION, ServerMessage};
pub use operations::{ErrorCode, PeerIdentity, TaskEvent, TaskEventKind, TaskSummary};
