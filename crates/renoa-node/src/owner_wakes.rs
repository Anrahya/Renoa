//! Waking the owner's phone for what the Host has for them.
//!
//! The Host decides which feed items wake the phone and whether they ring;
//! this loop delivers. Every [`PAUSE`] it reads the items still owed a wake,
//! oldest first, and asks the coordinator for each over the node's own
//! session, under the item's id. The coordinator acknowledges only once the
//! wake is durable, and the loop then records it on the item. A lost
//! acknowledgement leaves the item owed, so the same id is asked for again and
//! the coordinator records it once. While the session is down the feed is not
//! read; a refusal or a feed that cannot be read is logged and holds the loop
//! back for [`HELD_BACK`]. Nothing it meets ends the node.

use std::{sync::Arc, time::Duration};

use renoa_control::WakeUrgency;
use renoa_local::{FeedWake, HostFeed, TurnObservation};
use tokio_util::sync::CancellationToken;

use crate::{
    agent_link::{AgentLink, WakeFailure},
    node_log,
};

/// How long the loop waits before reading the feed again.
const PAUSE: Duration = Duration::from_secs(1);
/// How long a refusal or an unreadable feed holds the loop back, so neither
/// is asked about or logged every second.
const HELD_BACK: Duration = Duration::from_mins(1);

/// Runs until `shutdown`.
pub(crate) async fn run(feed: HostFeed, link: Arc<AgentLink>, shutdown: CancellationToken) {
    loop {
        let pause = pass(&feed, &link).await;
        tokio::select! {
            () = shutdown.cancelled() => return,
            () = tokio::time::sleep(pause) => {}
        }
    }
}

/// Asks for every owed wake it can, and returns how long to wait before the
/// next pass.
async fn pass(feed: &HostFeed, link: &AgentLink) -> Duration {
    if !link.is_open() {
        return PAUSE;
    }
    let wakes = match feed.owed_wakes().await {
        Ok(wakes) => wakes,
        Err(error) => {
            failed("feed_read", &error.to_string());
            return HELD_BACK;
        }
    };
    for FeedWake { id, rings } in wakes {
        let urgency = if rings {
            WakeUrgency::Now
        } else {
            WakeUrgency::Soon
        };
        match link.request_wake(id, urgency).await {
            Ok(()) => {}
            // The session is down or did not answer; the next pass asks again.
            Err(WakeFailure::Unanswered) => return PAUSE,
            Err(WakeFailure::Refused { code, message }) => {
                node_log::event(
                    "warn",
                    "owner_wake_refused",
                    &serde_json::json!({ "wake_id": id, "code": code, "message": message }),
                );
                return HELD_BACK;
            }
        }
        let recorded = match TurnObservation::now() {
            Ok(now) => feed
                .wake_sent(id, now.unix_milliseconds())
                .await
                .map_err(|error| error.to_string()),
            Err(error) => Err(error.to_string()),
        };
        if let Err(error) = recorded {
            failed("feed_write", &error);
            return HELD_BACK;
        }
        node_log::event(
            "info",
            "owner_wake_requested",
            &serde_json::json!({ "wake_id": id, "urgency": urgency }),
        );
    }
    PAUSE
}

fn failed(reason: &str, error: &str) {
    node_log::event(
        "error",
        "owner_wake_failed",
        &serde_json::json!({ "reason": reason, "error": error }),
    );
}
