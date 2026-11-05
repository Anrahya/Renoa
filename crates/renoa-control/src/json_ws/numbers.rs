//! Which frames carry only numbers every JSON implementation reads exactly:
//! unsigned values up to 2^53 - 1 and signed timestamps within the same bound.

use renoa_protocol::{ExecutionEvent, LiveActivity};

use super::{ClientMessage, ServerMessage};
use crate::TaskEventKind;

pub(super) const MAX_INTEROPERABLE_INTEGER: u64 = 9_007_199_254_740_991;
const MAX_INTEROPERABLE_SIGNED_INTEGER: i64 = 9_007_199_254_740_991;

impl ClientMessage {
    /// The `request_id` of a frame that is not a valid client message, when
    /// it is an object carrying one as an interoperable integer, so the
    /// refusal names the request the surface sent.
    pub(crate) fn refused_request_id(json: &str) -> Option<u64> {
        #[derive(serde::Deserialize)]
        struct Envelope {
            request_id: u64,
        }
        // A derived struct also reads a JSON array, which is no frame.
        if !json.trim_start().starts_with('{') {
            return None;
        }
        serde_json::from_str::<Envelope>(json)
            .ok()
            .map(|envelope| envelope.request_id)
            .filter(|request_id| interoperable(*request_id))
    }

    pub(crate) fn has_interoperable_numbers(&self) -> bool {
        match self {
            Self::Enroll { .. }
            | Self::Authenticate { .. }
            | Self::AuthenticateTicket { .. }
            | Self::DeviceProof { .. }
            | Self::AdvertiseTargets { .. }
            | Self::AcknowledgeExecution { .. }
            | Self::AcknowledgeStop { .. }
            | Self::ServeHostControl
            | Self::CompleteHostCall { .. } => true,
            Self::ListTasks { request_id }
            | Self::ListTargets { request_id }
            | Self::ListDevices { request_id }
            | Self::RevokeDevice { request_id, .. }
            | Self::OpenTask { request_id, .. }
            | Self::Submit { request_id, .. }
            | Self::StopCommand { request_id, .. }
            | Self::DeleteTask { request_id, .. }
            | Self::CallHost { request_id, .. }
            | Self::SetPushToken { request_id, .. }
            | Self::ClearPushToken { request_id }
            | Self::SendAgentMessage { request_id, .. }
            | Self::RequestWake { request_id, .. }
            | Self::SetNoticeKey { request_id, .. }
            | Self::ListNotices { request_id }
            | Self::AcknowledgeNotice { request_id, .. }
            | Self::ListNoticeKeys { request_id }
            | Self::WithdrawNotices { request_id, .. } => interoperable(*request_id),
            Self::PutNotice { request_id, notice } => {
                interoperable(*request_id) && timestamp(notice.expires_at_ms)
            }
            Self::Attach {
                request_id,
                after_sequence,
                ..
            } => interoperable(*request_id) && after_sequence.is_none_or(interoperable),
            Self::PublishExecutionEvents { events, .. } => {
                events.iter().all(execution_event_has_interoperable_numbers)
            }
            Self::PublishLiveActivity { activity, .. } => {
                live_activity_has_interoperable_numbers(activity)
            }
        }
    }
}

impl ServerMessage {
    pub(crate) fn has_interoperable_numbers(&self) -> bool {
        match self {
            Self::Enrolled { .. }
            | Self::Authenticated { .. }
            | Self::DeviceChallenge { .. }
            | Self::ExecutionAcknowledged { .. }
            | Self::Execute { .. }
            | Self::Stop { .. }
            | Self::HostCall { .. } => true,
            Self::TaskList { request_id, tasks } => {
                interoperable(*request_id)
                    && tasks
                        .iter()
                        .all(|task| task.through_sequence.is_none_or(interoperable))
            }
            Self::TargetList { request_id, .. }
            | Self::DeviceRevoked { request_id, .. }
            | Self::TaskOpened { request_id, .. }
            | Self::CommandAccepted { request_id, .. }
            | Self::StopAccepted { request_id, .. }
            | Self::TaskDeleted { request_id, .. }
            | Self::HostAnswer { request_id, .. }
            | Self::PushTokenSet { request_id }
            | Self::PushTokenCleared { request_id }
            | Self::WakeRequested { request_id }
            | Self::NoticeKeySet { request_id }
            | Self::NoticeAcknowledged { request_id }
            | Self::NoticeKeys { request_id, .. }
            | Self::NoticePut { request_id }
            | Self::NoticesWithdrawn { request_id } => interoperable(*request_id),
            Self::NoticeList {
                request_id,
                notices,
            } => {
                interoperable(*request_id)
                    && notices.iter().all(|notice| timestamp(notice.expires_at_ms))
            }
            Self::DeviceList {
                request_id,
                devices,
            } => {
                interoperable(*request_id)
                    && devices
                        .iter()
                        .all(|device| interoperable(device.created_at_ms))
            }
            Self::Attached {
                request_id,
                through_sequence,
                ..
            } => interoperable(*request_id) && through_sequence.is_none_or(interoperable),
            Self::ExecutionEventsAccepted {
                through_execution_sequence,
                ..
            } => interoperable(*through_execution_sequence),
            Self::TaskEvent { event } => {
                interoperable(event.sequence)
                    && match &event.kind {
                        TaskEventKind::CommandSubmitted { .. } => true,
                        TaskEventKind::ExecutionEvent { event, .. } => {
                            execution_event_has_interoperable_numbers(event)
                        }
                    }
            }
            Self::LiveActivity { activity, .. } => {
                live_activity_has_interoperable_numbers(activity)
            }
            Self::Error { request_id, .. } => request_id.is_none_or(interoperable),
        }
    }
}

fn live_activity_has_interoperable_numbers(activity: &LiveActivity) -> bool {
    match activity {
        LiveActivity::TextDelta { sequence, .. } => interoperable(*sequence),
        LiveActivity::TextReset => true,
    }
}

const fn interoperable(value: u64) -> bool {
    value <= MAX_INTEROPERABLE_INTEGER
}

fn execution_event_has_interoperable_numbers(event: &ExecutionEvent) -> bool {
    interoperable(event.sequence) && timestamp(event.recorded_at_ms)
}

fn timestamp(value: i64) -> bool {
    (-MAX_INTEROPERABLE_SIGNED_INTEGER..=MAX_INTEROPERABLE_SIGNED_INTEGER).contains(&value)
}
