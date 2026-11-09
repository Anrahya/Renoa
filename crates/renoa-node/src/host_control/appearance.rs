//! How an agent looks: the look the Host picked for it, and a picture the
//! owner sets over it. An app reads the appearance, and reads the picture
//! again only when its revision changed.

use base64::{Engine as _, engine::general_purpose::STANDARD};
use renoa_local::{LocalHost, MAX_PICTURE_BYTES, NewPicture, PictureChange, PictureError};
use serde::Deserialize;
use serde_json::json;
use uuid::Uuid;

use super::{
    BODY_LIMIT,
    reply::{CONFLICT, NOT_FOUND, Reply, UNAVAILABLE, UNPROCESSABLE, agent_id, log},
};

/// A picture in base64, with the change's other fields beside it.
pub(super) const PICTURE_BODY_LIMIT: usize = MAX_PICTURE_BYTES.div_ceil(3) * 4 + BODY_LIMIT;
pub(super) const PICTURE_HINT: &str = "Send an operation ID, the expected_revision of the picture you are replacing \
     (null for none), and the picture as {\"content_type\", \"data\"} in base64, or null to take it away.";

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Change {
    operation_id: Uuid,
    expected_revision: Option<String>,
    picture: Option<Picture>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Picture {
    content_type: String,
    data: String,
}

pub(super) async fn read(host: &LocalHost, id: &str) -> Reply {
    let agent = match agent_id(id) {
        Ok(agent) => agent,
        Err(reply) => return reply,
    };
    match host.agent_appearance(agent).await {
        Ok(Some(appearance)) => Reply::json(&appearance),
        Ok(None) => Reply::failure(NOT_FOUND, "not_found", "This agent no longer exists."),
        Err(error) => unavailable(&error.to_string()),
    }
}

pub(super) async fn picture(host: &LocalHost, id: &str) -> Reply {
    let agent = match agent_id(id) {
        Ok(agent) => agent,
        Err(reply) => return reply,
    };
    match host.agent_picture(agent).await {
        Ok(Some(picture)) => Reply::json(&json!({
            "revision": picture.revision,
            "content_type": picture.content_type,
            "data": STANDARD.encode(picture.bytes),
        })),
        Ok(None) => Reply::failure(NOT_FOUND, "picture_not_found", "This agent has no picture."),
        Err(error) => unavailable(&error.to_string()),
    }
}

pub(super) async fn set_picture(host: &LocalHost, id: &str, change: Change) -> Reply {
    let agent = match agent_id(id) {
        Ok(agent) => agent,
        Err(reply) => return reply,
    };
    let picture = match change.picture.map(decode).transpose() {
        Ok(picture) => picture,
        Err(reply) => return reply,
    };
    let change = PictureChange {
        operation_id: change.operation_id,
        expected_revision: change.expected_revision,
        picture,
    };
    match host.set_agent_picture(agent, change).await {
        Ok(Some(revision)) => Reply::json(&json!({ "picture_revision": revision })),
        Ok(None) => Reply::failure(NOT_FOUND, "not_found", "This agent no longer exists."),
        Err(PictureError::Invalid(reason)) => {
            Reply::failure(UNPROCESSABLE, "invalid_picture", &reason)
        }
        Err(error @ PictureError::Stale { .. }) => {
            Reply::failure(CONFLICT, "revision_conflict", &error.to_string())
        }
        Err(PictureError::OperationReused(_)) => Reply::failure(
            CONFLICT,
            "operation_reused",
            "This operation ID was used for another change. Send a new one.",
        ),
        Err(error) => unavailable(&error.to_string()),
    }
}

fn decode(picture: Picture) -> Result<NewPicture, Reply> {
    let bytes = STANDARD.decode(picture.data).map_err(|_| {
        Reply::failure(
            UNPROCESSABLE,
            "invalid_picture",
            "The picture's data is not base64; send the file's bytes in standard base64.",
        )
    })?;
    Ok(NewPicture {
        content_type: picture.content_type,
        bytes,
    })
}

fn unavailable(error: &str) -> Reply {
    log("appearance", error);
    Reply::failure(
        UNAVAILABLE,
        "host_unavailable",
        "The agent's look could not be read or changed. Retry the same request.",
    )
}
