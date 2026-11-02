//! The owner's feed: one page of what happened, the Today line, and the read
//! state every device shares.

use renoa_local::{FeedError, HostFeed, LocalHost, TurnObservation};
use serde::Deserialize;
use uuid::Uuid;

use super::reply::{BAD_REQUEST, NOT_FOUND, Reply, UNAVAILABLE, UNPROCESSABLE, log};

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Seen {
    through: u64,
}

pub(super) async fn list(host: &LocalHost, before: Option<&str>) -> Reply {
    let Ok(before) = before.map(str::parse::<i64>).transpose() else {
        return Reply::failure(
            BAD_REQUEST,
            "invalid_request",
            "A page starts before a feed sequence, such as /v1/host/feed/before/120.",
        );
    };
    match open(host) {
        Ok(feed) => answer("feed list", feed.list(before).await),
        Err(reply) => reply,
    }
}

pub(super) async fn see(host: &LocalHost, seen: Seen) -> Reply {
    match open(host) {
        Ok(feed) => answer("feed see", feed.see(seen.through).await),
        Err(reply) => reply,
    }
}

pub(super) async fn dismiss(host: &LocalHost, id: &str) -> Reply {
    let Ok(id) = Uuid::parse_str(id) else {
        return Reply::failure(
            BAD_REQUEST,
            "invalid_request",
            "A feed item id is a UUID, such as 0f1e2d3c-4b5a-6978-8796-a5b4c3d2e1f0.",
        );
    };
    let now = match TurnObservation::now() {
        Ok(now) => now.unix_milliseconds(),
        Err(error) => {
            log("feed clock", &error.to_string());
            return Reply::failure(
                UNAVAILABLE,
                "clock_unavailable",
                "The Host clock is unavailable. Retry the same request.",
            );
        }
    };
    match open(host) {
        Ok(feed) => answer("feed dismiss", feed.dismiss(id, now).await),
        Err(reply) => reply,
    }
}

fn open(host: &LocalHost) -> Result<HostFeed, Reply> {
    HostFeed::open(host.home().path()).map_err(|error| unavailable("feed open", &error))
}

fn answer<T: serde::Serialize>(operation: &str, outcome: Result<T, FeedError>) -> Reply {
    match outcome {
        Ok(value) => Reply::json(&value),
        Err(error @ FeedError::NotFound(_)) => {
            Reply::failure(NOT_FOUND, "not_found", &error.to_string())
        }
        Err(error @ FeedError::AheadOfFeed { .. }) => {
            Reply::failure(UNPROCESSABLE, "ahead_of_feed", &error.to_string())
        }
        Err(error) => unavailable(operation, &error),
    }
}

fn unavailable(operation: &str, error: &FeedError) -> Reply {
    log(operation, &error.to_string());
    Reply::failure(
        UNAVAILABLE,
        "host_unavailable",
        "The feed is temporarily unavailable. Retry the same request.",
    )
}
