//! Cross-surface task continuity outside Renoa's execution kernel.

mod agent_messages;
mod agent_replies;
mod android_apps;
mod browser_identity;
mod browser_identity_http;
mod browser_identity_store;
mod browser_pairing_store;
mod browser_session_schema;
mod browser_sessions;
mod browser_sessions_http;
mod browser_ticket_store;
mod command_stop;
mod connection;
mod control_log;
mod control_migrations;
mod control_room;
mod control_schema;
mod coordinator;
mod credential_relay_http;
mod credential_relay_store;
mod device_admin;
mod device_handshake;
mod device_key;
mod device_store;
mod dispatch_store;
mod event_store;
mod fcm;
mod google_access;
mod host_calls;
mod host_control_http;
mod host_control_rcp;
mod identity;
mod identity_store;
mod json_ws;
mod live_activity;
mod node_messages;
mod node_ownership;
mod oauth_relay_http;
mod oauth_relay_store;
mod operations;
mod push_dispatch;
mod push_pacing;
mod push_supervisor;
mod push_tokens;
mod push_wake;
mod session_registry;
mod setup_notices;
mod share_links;
mod share_page_http;
mod store;
mod surface_operations;
mod task_deletion;
mod task_opening;
mod task_status;
mod wire;

pub use android_apps::AndroidApps;
pub use browser_sessions::{BrowserSession, BrowserSessions};
pub use coordinator::{ControlError, Coordinator, TaskSpec};
pub use fcm::PushSettings;
pub use identity::{BrowserPairingToken, PasskeyBootstrapToken};
pub use operations::PeerIdentity;
pub use setup_notices::{MAX_LISTED_NOTICES, MAX_SEALED_NOTICE_BYTES};

// The binding's frames and records belong to `renoa-rcp-wire`; the crate root
// names them so every module reaches them as `crate::…`.
use renoa_rcp_wire::{
    ClientMessage, ConnectionTicket, DeviceCredential, DeviceCredentials, DeviceId,
    DeviceNoticeKey, DeviceSummary, EnrollmentToken, ErrorCode, HeldNotice, HostMethod,
    JSON_WS_VERSION, MAX_APPLICATION_MESSAGE_BYTES, NodeId, NoticeKey, PushToken, SealedNotice,
    ServerMessage, ShareLink, StopOutcome, TargetSummary, TaskEvent, TaskEventId, TaskEventKind,
    TaskId, TaskStatus, TaskSummary, WakeUrgency,
};
