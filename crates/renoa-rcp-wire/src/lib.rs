//! The RCP JSON WebSocket binding's frames and the records they carry, shared
//! by the coordinator and every Rust client.
//!
//! It holds the frames' shapes and the checks a frame needs before either end
//! sends it. What a frame asks the coordinator to do, and who may ask it,
//! belongs to `renoa-control`; how a client connects belongs to
//! `renoa-rcp-client`.

mod device_sign_in;
mod ids;
mod messages;
mod notices;
mod records;
mod secrets;

pub use device_sign_in::{
    ApprovedCredentials, AwaitingApproval, CollectDeviceRequest, DEVICE_REQUEST_COLLECT_PATH,
    DEVICE_REQUESTS_PATH, DeviceRequest, DeviceRequestCode, IdentityRefusal, RequestingDevice,
    device_proof_message, device_request_message,
};
pub use ids::{DeviceId, NodeId, TaskEventId, TaskId};
pub use messages::{ClientMessage, JSON_WS_VERSION, MAX_APPLICATION_MESSAGE_BYTES, ServerMessage};
pub use notices::{DeviceNoticeKey, HeldNotice, NoticeKey, SealedNotice};
pub use records::{
    DeviceSummary, ErrorCode, HostMethod, StopOutcome, TargetSummary, TaskEvent, TaskEventKind,
    TaskStatus, TaskSummary, WakeUrgency,
};
pub use secrets::{
    ConnectionTicket, DeviceCode, DeviceCredential, DeviceCredentials, DeviceRequestProof,
    EnrollmentToken, PushToken, ShareLink,
};
