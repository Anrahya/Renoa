//! `/s/{share}`: a file an agent shared, opened like any web page. A browser
//! signed in to the Host reads it with its sign-in; an app opens the one-time
//! link `share_link` gave it (`share_links`). The coordinator reads the file
//! from the Host part by part and passes each part on as it arrives, keeping
//! none of it. Reading the Host's share description and parts is the one Host
//! operation the coordinator understands (locked decision 21 in `rcp-v0.md`).
//! A page runs sandboxed in an opaque origin, so it cannot act as the person
//! who opened it.

use std::{fmt::Write as _, io, sync::Arc, time::SystemTime};

use axum::{
    Router,
    body::{Body, Bytes},
    extract::{Path, State},
    http::{HeaderMap, HeaderValue, Method, StatusCode, Uri, header},
    response::Response,
    routing::get,
};
use base64::{Engine as _, engine::general_purpose::STANDARD};
use renoa_protocol::PrincipalId;
use serde::Deserialize;
use sha2::{Digest as _, Sha256};
use uuid::Uuid;

use crate::{
    HostMethod, control_log,
    coordinator::CoordinatorState,
    host_calls::{HostReply, HostRequest, call},
    identity::{Secret as _, ShareTicket},
};

/// How a shared file may run: scripts only, in an opaque origin, never the
/// coordinator's, so it reaches neither the sign-in cookie nor `/v1/host`. No
/// forms, popups or dialogs: a page under this address must not pass for one
/// of Renoa's own that asks for a secret.
pub(crate) const SANDBOX: &str = "sandbox allow-scripts; frame-ancestors 'none'";

/// A PDF is shown by the browser's own viewer, which refuses a sandboxed
/// document; it runs no script of the page's.
pub(crate) const PDF: &str = "frame-ancestors 'none'";

const NOT_SHARED: &str = "Nothing is shared at this address any more.";
const SIGN_IN: &str = "Sign in to Renoa in this browser, then open the file again from Renoa.";
const TICKET_USED: &str =
    "This link was already opened or has expired. Open the file again from the Renoa app.";

pub(crate) fn routes() -> Router<Arc<CoordinatorState>> {
    Router::new().route("/s/{share}", get(open))
}

async fn open(
    State(state): State<Arc<CoordinatorState>>,
    method: Method,
    Path(share): Path<String>,
    uri: Uri,
    headers: HeaderMap,
) -> Response {
    // A HEAD would use up a one-time link and send nothing.
    if method != Method::GET {
        return page(StatusCode::METHOD_NOT_ALLOWED, "Open this link with GET.");
    }
    let Some(share) = Uuid::parse_str(&share)
        .ok()
        .filter(|parsed| parsed.to_string() == share)
    else {
        return page(StatusCode::NOT_FOUND, NOT_SHARED);
    };
    let ticket = uri.query().and_then(|query| {
        query
            .split('&')
            .find_map(|pair| pair.strip_prefix("ticket="))
    });
    let now = SystemTime::now();
    let reader = match reader(&state, share, ticket, &headers, now).await {
        Ok(reader) => reader,
        Err(response) => return *response,
    };
    serve(state, share, reader, now).await
}

/// Who reads the share.
enum Reader {
    /// A one-time link's principal; the link is used up only once the file
    /// is known readable, so a Host that is briefly away costs no link.
    Link(PrincipalId, ShareTicket),
    /// The browser's signed-in principal, with its refreshed cookie.
    Browser(PrincipalId, Option<HeaderValue>),
}

impl Reader {
    const fn principal(&self) -> PrincipalId {
        match self {
            Self::Link(principal, _) | Self::Browser(principal, _) => *principal,
        }
    }
}

async fn reader(
    state: &CoordinatorState,
    share: Uuid,
    ticket: Option<&str>,
    headers: &HeaderMap,
    now: SystemTime,
) -> Result<Reader, Box<Response>> {
    if let Some(ticket) = ticket {
        let Some(ticket) = ShareTicket::from_encoded(ticket) else {
            return Err(Box::new(page(StatusCode::UNAUTHORIZED, TICKET_USED)));
        };
        return match state.store.share_ticket_reader(share, &ticket, now).await {
            Ok(Some(principal)) => Ok(Reader::Link(principal, ticket)),
            Ok(None) => Err(Box::new(page(StatusCode::UNAUTHORIZED, TICKET_USED))),
            Err(error) => Err(Box::new(unavailable(
                "share_ticket_failed",
                &error.to_string(),
            ))),
        };
    }
    match state.browser_sessions.authenticate(headers, now).await {
        Ok(Some(session)) => Ok(Reader::Browser(
            session.principal_id(),
            session.cookie(now).ok(),
        )),
        Ok(None) => Err(Box::new(page(StatusCode::UNAUTHORIZED, SIGN_IN))),
        Err(error) => Err(Box::new(unavailable(
            "share_session_failed",
            &error.to_string(),
        ))),
    }
}

/// What the Host says of a share before its parts are read.
#[derive(Deserialize)]
struct Description {
    name: String,
    media_type: String,
    size: u64,
    sha256: String,
    part_bytes: u64,
    parts: u64,
}

/// Reads the description and the first part before anything is sent, so the
/// failures a reader is most likely to meet are pages they can act on, and a
/// one-time link is used up only then.
async fn serve(
    state: Arc<CoordinatorState>,
    share: Uuid,
    reader: Reader,
    now: SystemTime,
) -> Response {
    let principal = reader.principal();
    let reply = match host(&state, principal, format!("/v1/host/shares/{share}")).await {
        Ok(reply) => reply,
        Err(response) => return *response,
    };
    let Some(described) = serde_json::from_str::<Description>(&reply.body)
        .ok()
        .filter(|described| {
            described.part_bytes > 0
                && described.parts == described.size.div_ceil(described.part_bytes)
                && described.sha256.len() == 64
        })
    else {
        return unavailable(
            "share_description_invalid",
            "the Host described a share badly",
        );
    };
    let mut reading = Reading {
        state,
        principal,
        share,
        next: 0,
        hash: Sha256::new(),
        described,
    };
    let first = if reading.described.parts == 0 {
        None
    } else {
        match reading.part().await {
            Ok(first) => Some(first),
            Err(PartError::Host(response)) => return *response,
            Err(PartError::Changed) => {
                return page(
                    StatusCode::CONFLICT,
                    "The file changed while it was opened. Open it again.",
                );
            }
        }
    };
    let cookie = match reader {
        Reader::Link(_, ticket) => {
            match reading
                .state
                .store
                .redeem_share_ticket(share, &ticket, now)
                .await
            {
                Ok(Some(_)) => None,
                Ok(None) => return page(StatusCode::UNAUTHORIZED, TICKET_USED),
                Err(error) => return unavailable("share_ticket_failed", &error.to_string()),
            }
        }
        Reader::Browser(_, cookie) => cookie,
    };
    let mut response = Response::new(Body::default());
    let headers = response.headers_mut();
    let described = &reading.described;
    headers.insert(header::CONTENT_TYPE, media_type(&described.media_type));
    headers.insert(header::CONTENT_LENGTH, HeaderValue::from(described.size));
    if let Ok(disposition) = HeaderValue::from_str(&format!(
        "inline; filename*=UTF-8''{}",
        percent_encoded(&described.name)
    )) {
        headers.insert(header::CONTENT_DISPOSITION, disposition);
    }
    let policy = if described.media_type == "application/pdf" {
        PDF
    } else {
        SANDBOX
    };
    protect(headers, policy);
    if let Some(cookie) = cookie {
        headers.insert(header::SET_COOKIE, cookie);
    }
    *response.body_mut() = Body::from_stream(futures_util::stream::try_unfold(
        (reading, first),
        |(mut reading, first)| async move {
            if let Some(first) = first {
                return Ok(Some((first, (reading, None))));
            }
            if reading.next == reading.described.parts {
                return Ok(None);
            }
            match reading.part().await {
                Ok(part) => Ok(Some((part, (reading, None)))),
                Err(_) => Err(io::Error::other("the shared file could not be read whole")),
            }
        },
    ));
    response
}

/// One read of a share's parts, in order. A part that fails or is the wrong
/// size, or a last part that leaves the file with another hash than the one
/// described, ends the body before that part: short of its Content-Length,
/// so no reader takes it as whole.
struct Reading {
    state: Arc<CoordinatorState>,
    principal: PrincipalId,
    share: Uuid,
    next: u64,
    hash: Sha256,
    described: Description,
}

enum PartError {
    /// The Host refused or could not be reached; its page.
    Host(Box<Response>),
    /// The part is not what the description promised.
    Changed,
}

impl Reading {
    /// The next part, checked against the description. The last part is
    /// returned only once the whole file matched its hash.
    async fn part(&mut self) -> Result<Bytes, PartError> {
        let path = format!("/v1/host/shares/{}/parts/{}", self.share, self.next);
        let reply = host(&self.state, self.principal, path)
            .await
            .map_err(|response| {
                cut_short(self.share, "a part could not be read");
                PartError::Host(response)
            })?;
        let described = &self.described;
        let data = serde_json::from_str::<serde_json::Value>(&reply.body)
            .ok()
            .and_then(|part| {
                part["data"]
                    .as_str()
                    .and_then(|data| STANDARD.decode(data).ok())
            })
            .ok_or_else(|| cut_short(self.share, "the Host sent a part badly"))?;
        let expected = if self.next + 1 == described.parts {
            described.size - described.part_bytes * self.next
        } else {
            described.part_bytes
        };
        if u64::try_from(data.len()).ok() != Some(expected) {
            return Err(cut_short(self.share, "the file changed while it was read"));
        }
        self.hash.update(&data);
        self.next += 1;
        if self.next == described.parts {
            let hex = std::mem::take(&mut self.hash).finalize().iter().fold(
                String::new(),
                |mut hex, byte| {
                    let _ = write!(hex, "{byte:02x}");
                    hex
                },
            );
            if hex != described.sha256 {
                return Err(cut_short(self.share, "the file changed while it was read"));
            }
        }
        Ok(Bytes::from(data))
    }
}

/// Logs why a read stopped; the part is then not sent.
fn cut_short(share: Uuid, reason: &str) -> PartError {
    control_log::event(
        "warn",
        "share_read_cut_short",
        &serde_json::json!({ "share_id": share, "reason": reason }),
    );
    PartError::Changed
}
/// The Host's answer to a GET, when it is a 200. Any other answer is the page
/// the reader sees: the Host's own message, under its status.
async fn host(
    state: &CoordinatorState,
    principal: PrincipalId,
    path: String,
) -> Result<HostReply, Box<Response>> {
    let request = HostRequest {
        method: HostMethod::Get,
        path,
        body: None,
    };
    match call(state, principal, request).await {
        Ok(reply) if reply.status == 200 => Ok(reply),
        Ok(reply) => {
            let message = serde_json::from_str::<serde_json::Value>(&reply.body)
                .ok()
                .and_then(|body| body["message"].as_str().map(str::to_owned))
                .unwrap_or_else(|| NOT_SHARED.to_owned());
            let status = StatusCode::from_u16(reply.status)
                .ok()
                .filter(|status| status.is_client_error() || status.is_server_error())
                .unwrap_or(StatusCode::BAD_GATEWAY);
            Err(Box::new(page(status, &message)))
        }
        Err(error) => Err(Box::new(page(
            StatusCode::SERVICE_UNAVAILABLE,
            error.message(),
        ))),
    }
}

/// The type the Host named, when it is a plain `type/subtype`, with text read
/// as UTF-8; any other is bytes to save.
fn media_type(named: &str) -> HeaderValue {
    let plain = named.split_once('/').is_some_and(|(kind, subtype)| {
        [kind, subtype].iter().all(|part| {
            !part.is_empty()
                && part.bytes().all(|byte| {
                    byte.is_ascii_lowercase() || byte.is_ascii_digit() || b"+-.".contains(&byte)
                })
        })
    });
    let named = if !plain {
        "application/octet-stream".to_owned()
    } else if named.starts_with("text/") {
        format!("{named}; charset=utf-8")
    } else {
        named.to_owned()
    };
    HeaderValue::from_str(&named).unwrap_or(HeaderValue::from_static("application/octet-stream"))
}

/// `name` as an RFC 5987 value: every byte but the unreserved ones escaped.
fn percent_encoded(name: &str) -> String {
    name.bytes()
        .map(|byte| {
            if byte.is_ascii_alphanumeric() || b"-._~".contains(&byte) {
                char::from(byte).to_string()
            } else {
                format!("%{byte:02X}")
            }
        })
        .collect()
}

fn protect(headers: &mut HeaderMap, policy: &'static str) {
    headers.insert(
        header::CACHE_CONTROL,
        HeaderValue::from_static("private, no-store, no-transform"),
    );
    headers.insert(
        header::REFERRER_POLICY,
        HeaderValue::from_static("no-referrer"),
    );
    headers.insert(
        header::X_CONTENT_TYPE_OPTIONS,
        HeaderValue::from_static("nosniff"),
    );
    headers.insert(
        "cross-origin-resource-policy",
        HeaderValue::from_static("same-origin"),
    );
    headers.insert(
        header::CONTENT_SECURITY_POLICY,
        HeaderValue::from_static(policy),
    );
}

/// A short page in plain words.
fn page(status: StatusCode, message: &str) -> Response {
    let mut response = Response::new(Body::from(message.to_owned()));
    *response.status_mut() = status;
    response.headers_mut().insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("text/plain; charset=utf-8"),
    );
    protect(
        response.headers_mut(),
        "default-src 'none'; frame-ancestors 'none'",
    );
    response
}

fn unavailable(event: &'static str, error: &str) -> Response {
    control_log::event("warn", event, &serde_json::json!({ "error": error }));
    page(
        StatusCode::SERVICE_UNAVAILABLE,
        "Renoa could not open this file just now. Try again in a moment.",
    )
}

#[cfg(test)]
mod tests;
