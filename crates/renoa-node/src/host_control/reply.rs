//! One Host control answer: an HTTP-shaped status and a JSON body, as the
//! coordinator relays it to the owner's browser.

use serde::{Serialize, de::DeserializeOwned};

use crate::node_log;

pub(super) const BAD_REQUEST: u16 = 400;
pub(super) const NOT_FOUND: u16 = 404;
pub(super) const METHOD_NOT_ALLOWED: u16 = 405;
pub(super) const CONFLICT: u16 = 409;
pub(super) const PAYLOAD_TOO_LARGE: u16 = 413;
pub(super) const UNPROCESSABLE: u16 = 422;
pub(super) const INTERNAL: u16 = 500;
pub(super) const BAD_GATEWAY: u16 = 502;
pub(super) const UNAVAILABLE: u16 = 503;

#[derive(Debug)]
pub(crate) struct Reply {
    pub(crate) status: u16,
    pub(crate) body: String,
}

impl Reply {
    pub(super) fn json<T: Serialize>(value: &T) -> Self {
        match serde_json::to_string(value) {
            Ok(body) => Self { status: 200, body },
            Err(error) => {
                log("serialization", &error.to_string());
                Self::failure(
                    INTERNAL,
                    "internal",
                    "The Host could not encode its answer.",
                )
            }
        }
    }

    pub(crate) fn failure(status: u16, code: &str, message: &str) -> Self {
        Self {
            status,
            body: serde_json::json!({ "code": code, "message": message }).to_string(),
        }
    }

    /// The answer when the real one is too large for one relay message.
    pub(crate) fn too_large() -> Self {
        Self::failure(
            BAD_GATEWAY,
            "host_reply_too_large",
            "Your Host's answer is too large to relay.",
        )
    }
}

/// Records why an operation failed. The owner sees only the fixed message.
pub(super) fn log(operation: &str, error: &str) {
    node_log::event(
        "warn",
        "host_control_failed",
        &serde_json::json!({ "operation": operation, "error": error }),
    );
}

/// Reads a request body that must be present, within `limit` bytes, and match
/// `T`. `hint` is the owner-facing message when the body is not that shape.
pub(super) fn decode<T: DeserializeOwned>(
    body: Option<&str>,
    limit: usize,
    hint: &str,
) -> Result<T, Reply> {
    let Some(body) = body else {
        return Err(Reply::failure(BAD_REQUEST, "invalid_request", hint));
    };
    if body.len() > limit {
        return Err(Reply::failure(
            PAYLOAD_TOO_LARGE,
            "body_too_large",
            "The request body is too large.",
        ));
    }
    serde_json::from_str(body).map_err(|error| {
        let status = if error.is_data() {
            UNPROCESSABLE
        } else {
            BAD_REQUEST
        };
        Reply::failure(status, "invalid_request", hint)
    })
}
