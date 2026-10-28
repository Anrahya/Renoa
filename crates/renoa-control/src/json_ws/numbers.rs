//! Which frames carry only numbers every JSON implementation reads exactly:
//! unsigned values up to 2^53 - 1 and signed timestamps within the same bound.

use renoa_protocol::{ExecutionEvent, LiveActivity};

use super::{ClientMessage, ServerMessage};
use crate::TaskEventKind;

pub(super) const MAX_INTEROPERABLE_INTEGER: u64 = 9_007_199_254_740_991;
const MAX_INTEROPERABLE_SIGNED_INTEGER: i64 = 9_007_199_254_740_991;

impl ClientMessage {
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
            | Self::SendAgentMessage { request_id, .. } => interoperable(*request_id),
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
            Self::TaskList { request_id, .. }
            | Self::TargetList { request_id, .. }
            | Self::DeviceRevoked { request_id, .. }
            | Self::TaskOpened { request_id, .. }
            | Self::CommandAccepted { request_id, .. }
            | Self::StopAccepted { request_id, .. }
            | Self::TaskDeleted { request_id, .. }
            | Self::HostAnswer { request_id, .. } => interoperable(*request_id),
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
    interoperable(event.sequence)
        && (-MAX_INTEROPERABLE_SIGNED_INTEGER..=MAX_INTEROPERABLE_SIGNED_INTEGER)
            .contains(&event.recorded_at_ms)
}
