//! The frames of the JSON WebSocket binding, `docs/rcp-json-ws-v0.md`.

use renoa_protocol::{
    CommandEnvelope, CommandId, CommandInput, ExecutionEvent, LiveActivity, PrincipalId,
    SurfaceRef, TargetCard, TargetRef,
};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::{
    ConnectionTicket, DeviceCode, DeviceCredentials, DeviceId, DeviceNoticeKey, DeviceSummary,
    EnrollmentToken, ErrorCode, HeldNotice, HostMethod, NodeId, NoticeKey, PushToken, SealedNotice,
    ShareLink, StopOutcome, TargetSummary, TaskEvent, TaskId, TaskSummary, WakeUrgency,
};

pub const JSON_WS_VERSION: u32 = 18;

/// The binding's largest reassembled application message.
pub const MAX_APPLICATION_MESSAGE_BYTES: usize = 1024 * 1024;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ClientMessage {
    Enroll {
        version: u32,
        token: EnrollmentToken,
    },
    Authenticate {
        version: u32,
        credentials: DeviceCredentials,
    },
    AuthenticateTicket {
        version: u32,
        ticket: ConnectionTicket,
    },
    /// The signature a key-bound device answers its `device_challenge` with.
    DeviceProof {
        signature: String,
    },
    ListTasks {
        request_id: u64,
    },
    ListTargets {
        request_id: u64,
    },
    ListDevices {
        request_id: u64,
    },
    RevokeDevice {
        request_id: u64,
        device_id: DeviceId,
    },
    OpenTask {
        request_id: u64,
        task_id: TaskId,
        node_id: NodeId,
        target: TargetRef,
    },
    Attach {
        request_id: u64,
        task_id: TaskId,
        after_sequence: Option<u64>,
        /// Whether the surface also wants the task's ephemeral live activity.
        live: bool,
    },
    Submit {
        request_id: u64,
        task_id: TaskId,
        command_id: CommandId,
        input: CommandInput,
    },
    StopCommand {
        request_id: u64,
        task_id: TaskId,
        command_id: CommandId,
    },
    DeleteTask {
        request_id: u64,
        task_id: TaskId,
    },
    /// The owner's Host control call, from a signed-in app or browser.
    CallHost {
        request_id: u64,
        method: HostMethod,
        path: String,
        body: Option<String>,
    },
    /// A one-time link to a file an agent shared, from a signed-in app.
    ShareLink {
        request_id: u64,
        share_id: Uuid,
    },
    /// Signs in the computer showing `code` as this app's principal.
    ApproveDevice {
        request_id: u64,
        code: DeviceCode,
    },
    /// The device's push token, from an app that signed in. `provider` stays
    /// text on the wire so an unsupported one is answered by name.
    SetPushToken {
        request_id: u64,
        provider: String,
        token: PushToken,
    },
    ClearPushToken {
        request_id: u64,
    },
    /// The key a signed-in app opens its setup notices with.
    SetNoticeKey {
        request_id: u64,
        public_key: NoticeKey,
    },
    ListNotices {
        request_id: u64,
    },
    AcknowledgeNotice {
        request_id: u64,
        notice_id: Uuid,
    },
    AdvertiseTargets {
        targets: Vec<TargetCard>,
    },
    PublishExecutionEvents {
        task_id: TaskId,
        command_id: CommandId,
        events: Vec<ExecutionEvent>,
    },
    AcknowledgeExecution {
        task_id: TaskId,
        command_id: CommandId,
    },
    AcknowledgeStop {
        task_id: TaskId,
        command_id: CommandId,
    },
    PublishLiveActivity {
        task_id: TaskId,
        command_id: CommandId,
        activity: LiveActivity,
    },
    /// The node answers Host control calls for the owner who enrolled it.
    ServeHostControl,
    CompleteHostCall {
        call_id: Uuid,
        status: u16,
        body: String,
    },
    SendAgentMessage {
        request_id: u64,
        from_command: CommandId,
        message_id: CommandId,
        node_id: NodeId,
        target: TargetRef,
        text: String,
    },
    /// The node asks to wake its owner's phone for `wake_id`, something it has
    /// for them that no journal record carries.
    RequestWake {
        request_id: u64,
        wake_id: Uuid,
        urgency: WakeUrgency,
    },
    /// The node asks for its owner's devices it may seal a setup notice to.
    ListNoticeKeys {
        request_id: u64,
    },
    PutNotice {
        request_id: u64,
        notice: SealedNotice,
    },
    WithdrawNotices {
        request_id: u64,
        notice_ids: Vec<Uuid>,
    },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ServerMessage {
    Enrolled {
        version: u32,
        credentials: DeviceCredentials,
    },
    Authenticated {
        version: u32,
    },
    /// Sent instead of `authenticated` to a device that registered a key.
    DeviceChallenge {
        nonce: String,
    },
    TaskList {
        request_id: u64,
        tasks: Vec<TaskSummary>,
    },
    TargetList {
        request_id: u64,
        targets: Vec<TargetSummary>,
    },
    DeviceList {
        request_id: u64,
        devices: Vec<DeviceSummary>,
    },
    DeviceRevoked {
        request_id: u64,
        device_id: DeviceId,
    },
    TaskOpened {
        request_id: u64,
        task_id: TaskId,
    },
    Attached {
        request_id: u64,
        task_id: TaskId,
        through_sequence: Option<u64>,
    },
    CommandAccepted {
        request_id: u64,
        command_id: CommandId,
    },
    StopAccepted {
        request_id: u64,
        command_id: CommandId,
        outcome: StopOutcome,
    },
    TaskDeleted {
        request_id: u64,
        task_id: TaskId,
    },
    /// The Host's answer to a `call_host`, as its HTTP status and JSON body.
    HostAnswer {
        request_id: u64,
        status: u16,
        body: String,
    },
    /// A one-time link to the share: a path on the coordinator's own origin,
    /// usable once within two minutes.
    ShareLinkIssued {
        request_id: u64,
        path: ShareLink,
    },
    /// The computer that showed the code may now collect its credential.
    DeviceApproved {
        request_id: u64,
        surface: SurfaceRef,
        label: String,
    },
    PushTokenSet {
        request_id: u64,
    },
    PushTokenCleared {
        request_id: u64,
    },
    /// The wake a node asked for is durably recorded, or was already.
    WakeRequested {
        request_id: u64,
    },
    NoticeKeySet {
        request_id: u64,
    },
    /// The device's unexpired setup notices.
    NoticeList {
        request_id: u64,
        notices: Vec<HeldNotice>,
    },
    NoticeAcknowledged {
        request_id: u64,
    },
    /// The node owner's active devices with a notice key.
    NoticeKeys {
        request_id: u64,
        keys: Vec<DeviceNoticeKey>,
    },
    /// The notice is stored, or its id was stored already.
    NoticePut {
        request_id: u64,
    },
    NoticesWithdrawn {
        request_id: u64,
    },
    ExecutionEventsAccepted {
        command_id: CommandId,
        through_execution_sequence: u64,
    },
    ExecutionAcknowledged {
        command_id: CommandId,
    },
    Execute {
        task_id: TaskId,
        command: CommandEnvelope,
    },
    Stop {
        task_id: TaskId,
        command_id: CommandId,
    },
    /// One owner call under `/v1/host`, for the node that serves the Host.
    HostCall {
        call_id: Uuid,
        principal_id: PrincipalId,
        method: HostMethod,
        path: String,
        body: Option<String>,
    },
    TaskEvent {
        event: TaskEvent,
    },
    LiveActivity {
        task_id: TaskId,
        command_id: CommandId,
        activity: LiveActivity,
    },
    Error {
        request_id: Option<u64>,
        code: ErrorCode,
        message: String,
    },
}

mod numbers;

#[cfg(test)]
mod tests;
