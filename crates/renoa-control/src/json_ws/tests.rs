//! The directions the socket tests cannot reach: the coordinator never
//! receives an unsafe number it would later send back, so the outbound
//! checks are exercised here.

use super::*;

const LARGEST_SAFE: u64 = MAX_INTEROPERABLE_INTEGER;
const SMALLEST_UNSAFE: u64 = MAX_INTEROPERABLE_INTEGER + 1;

fn delta(sequence: u64) -> LiveActivity {
    LiveActivity::TextDelta {
        sequence,
        text: "text".to_owned(),
    }
}

#[test]
fn a_live_frame_numbered_beyond_what_javascript_preserves_is_not_sent_or_accepted() {
    let published = |activity| ClientMessage::PublishLiveActivity {
        task_id: TaskId::new(),
        command_id: CommandId::new(),
        activity,
    };
    let relayed = |activity| ServerMessage::LiveActivity {
        task_id: TaskId::new(),
        command_id: CommandId::new(),
        activity,
    };

    assert!(published(delta(LARGEST_SAFE)).has_interoperable_numbers());
    assert!(!published(delta(SMALLEST_UNSAFE)).has_interoperable_numbers());
    assert!(published(LiveActivity::TextReset).has_interoperable_numbers());
    assert!(relayed(delta(LARGEST_SAFE)).has_interoperable_numbers());
    assert!(!relayed(delta(SMALLEST_UNSAFE)).has_interoperable_numbers());
    assert!(relayed(LiveActivity::TextReset).has_interoperable_numbers());
}

#[test]
fn a_stop_request_id_beyond_what_javascript_preserves_is_not_sent_or_accepted() {
    let requested = |request_id| ClientMessage::StopCommand {
        request_id,
        task_id: TaskId::new(),
        command_id: CommandId::new(),
    };
    let accepted = |request_id| ServerMessage::StopAccepted {
        request_id,
        command_id: CommandId::new(),
        outcome: StopOutcome::Requested,
    };

    assert!(requested(LARGEST_SAFE).has_interoperable_numbers());
    assert!(!requested(SMALLEST_UNSAFE).has_interoperable_numbers());
    assert!(accepted(LARGEST_SAFE).has_interoperable_numbers());
    assert!(!accepted(SMALLEST_UNSAFE).has_interoperable_numbers());
}
