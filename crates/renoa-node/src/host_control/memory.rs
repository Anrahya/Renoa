//! The notes agents keep about the signed-in owner, read only.

use renoa_local::LocalHost;
use renoa_protocol::PrincipalId;

use super::reply::{Reply, UNAVAILABLE, log};

pub(super) async fn read(host: &LocalHost, principal: PrincipalId) -> Reply {
    match host.memory_notes(principal.as_uuid()).await {
        Ok(notes) => Reply::json(&notes),
        Err(error) => {
            log("memory read", &error.to_string());
            Reply::failure(
                UNAVAILABLE,
                "memory_unavailable",
                "Your notes could not be read. Retry in a moment.",
            )
        }
    }
}
