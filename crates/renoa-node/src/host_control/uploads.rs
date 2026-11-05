//! The owner's uploads: a file sent in parts, in order, for a message to
//! attach. Starting the same upload or sending a stored part again changes
//! nothing, so a caller unsure of an answer sends the same request again.

use base64::{Engine as _, engine::general_purpose::STANDARD};
use renoa_local::{UPLOAD_PART_BYTES, UploadError, UploadStatus, Uploads};
use renoa_protocol::PrincipalId;
use serde::Deserialize;
use uuid::Uuid;

use super::reply::{BAD_REQUEST, CONFLICT, NOT_FOUND, Reply, UNAVAILABLE, UNPROCESSABLE, log};

/// The largest part body: a whole part in base64, and the JSON around it.
pub(super) const PART_BODY_LIMIT: usize = UPLOAD_PART_BYTES.div_ceil(3) * 4 + 64;

pub(super) const START_HINT: &str = "Send the file's size in bytes and its SHA-256 as JSON, such as {\"size\": 1024, \"sha256\": \"…\"}.";
pub(super) const PART_HINT: &str =
    "Send the part's bytes in base64 as JSON, such as {\"data\": \"…\"}.";

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Start {
    size: u64,
    sha256: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Part {
    data: String,
}

pub(super) async fn start(
    uploads: &Uploads,
    principal: PrincipalId,
    id: &str,
    start: Start,
) -> Reply {
    match upload_id(id) {
        Ok(id) => answer(
            uploads
                .start(principal.as_uuid(), id, start.size, start.sha256)
                .await,
        ),
        Err(reply) => reply,
    }
}

pub(super) async fn put_part(
    uploads: &Uploads,
    principal: PrincipalId,
    id: &str,
    index: &str,
    part: Part,
) -> Reply {
    let id = match upload_id(id) {
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
    let Ok(data) = STANDARD.decode(part.data) else {
        return Reply::failure(BAD_REQUEST, "invalid_request", PART_HINT);
    };
    answer(uploads.put_part(principal.as_uuid(), id, index, data).await)
}

fn upload_id(id: &str) -> Result<Uuid, Reply> {
    Uuid::parse_str(id).map_err(|_| {
        Reply::failure(
            BAD_REQUEST,
            "invalid_request",
            "An upload id is a UUID the app chooses, such as 0f1e2d3c-4b5a-6978-8796-a5b4c3d2e1f0.",
        )
    })
}

fn answer(outcome: Result<UploadStatus, UploadError>) -> Reply {
    match outcome {
        Ok(status) => Reply::json(&status),
        Err(UploadError::Invalid(reason)) => {
            Reply::failure(UNPROCESSABLE, "invalid_upload", &reason)
        }
        Err(error @ UploadError::NotFound) => {
            Reply::failure(NOT_FOUND, "upload_not_found", &error.to_string())
        }
        Err(UploadError::Conflict(reason)) => Reply::failure(CONFLICT, "upload_conflict", &reason),
        Err(error) => {
            log("uploads", &error.to_string());
            Reply::failure(
                UNAVAILABLE,
                "host_unavailable",
                "The upload could not be stored. Send the same request again.",
            )
        }
    }
}
