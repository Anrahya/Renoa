//! The files agents share with the owner: a share's description, then its
//! bytes in parts, read from the file as it is now. A surface checks the
//! parts it put together against the description's SHA-256, and starts again
//! when the file changed while it read.

use base64::{Engine as _, engine::general_purpose::STANDARD};
use renoa_local::{ShareError, Shares};
use renoa_protocol::PrincipalId;
use serde_json::json;
use uuid::Uuid;

use super::reply::{BAD_REQUEST, NOT_FOUND, PAYLOAD_TOO_LARGE, Reply, UNAVAILABLE, log};

pub(super) async fn describe(shares: &Shares, principal: PrincipalId, id: &str) -> Reply {
    match share_id(id) {
        Ok(id) => match shares.describe(id, principal.as_uuid()).await {
            Ok(description) => Reply::json(&description),
            Err(error) => failure(&error),
        },
        Err(reply) => reply,
    }
}

pub(super) async fn part(shares: &Shares, principal: PrincipalId, id: &str, index: &str) -> Reply {
    let id = match share_id(id) {
        Ok(id) => id,
        Err(reply) => return reply,
    };
    // One spelling per part: "01" and "+1" are not part 1.
    let Some(index) = index
        .parse::<u64>()
        .ok()
        .filter(|parsed| parsed.to_string() == index)
    else {
        return Reply::failure(
            BAD_REQUEST,
            "invalid_request",
            "A part number is a whole number from 0.",
        );
    };
    match shares.part(id, principal.as_uuid(), index).await {
        Ok(bytes) => Reply::json(&json!({ "data": STANDARD.encode(bytes) })),
        Err(error) => failure(&error),
    }
}

fn share_id(id: &str) -> Result<Uuid, Reply> {
    Uuid::parse_str(id).map_err(|_| {
        Reply::failure(
            BAD_REQUEST,
            "invalid_request",
            "A share id is the UUID at the end of its link.",
        )
    })
}

fn failure(error: &ShareError) -> Reply {
    match error {
        ShareError::NotFound => Reply::failure(NOT_FOUND, "share_not_found", &error.to_string()),
        ShareError::TooLarge { .. } => {
            Reply::failure(PAYLOAD_TOO_LARGE, "share_too_large", &error.to_string())
        }
        ShareError::NoSuchPart { .. } => {
            Reply::failure(NOT_FOUND, "share_part_not_found", &error.to_string())
        }
        _ => {
            log("shares", &error.to_string());
            Reply::failure(
                UNAVAILABLE,
                "host_unavailable",
                "The shared file could not be read. Try again in a moment.",
            )
        }
    }
}
