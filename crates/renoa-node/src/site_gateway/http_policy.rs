//! Shared HTTP header, URL validation, cookie isolation, and error-sanitization
//! policy for the live-app reverse proxy (`proxy` and `ws`).

use std::collections::HashSet;

use axum::{
    http::{HeaderMap, HeaderName, HeaderValue, StatusCode, header},
    response::{IntoResponse, Response},
};
use renoa_local::AppError;

use crate::node_log;

const RESERVED_COOKIE_PREFIX: &str = "__host-renoa_";
const X_FORWARDED_PREFIX: HeaderName = HeaderName::from_static("x-forwarded-prefix");
const X_FORWARDED_HOST: HeaderName = HeaderName::from_static("x-forwarded-host");
const X_FORWARDED_PROTO: HeaderName = HeaderName::from_static("x-forwarded-proto");

pub(super) fn error_response(status: StatusCode, body: impl Into<String>) -> Response {
    (
        status,
        [(header::X_CONTENT_TYPE_OPTIONS, "nosniff")],
        body.into(),
    )
        .into_response()
}

pub(super) fn app_error_response(name: &str, err: &AppError) -> Response {
    match err {
        AppError::NotFound | AppError::InvalidName => error_response(
            StatusCode::NOT_FOUND,
            "No live app is registered at this address.",
        ),
        AppError::Failed {
            name: app_name,
            reason,
            logs,
        } => {
            node_log::event(
                "warn",
                "live_app_gateway_failed",
                &serde_json::json!({ "app": app_name, "reason": reason, "logs": logs }),
            );
            error_response(StatusCode::BAD_GATEWAY, "Upstream live app is unavailable.")
        }
        _ => sanitize_gateway_failure(name, &err.to_string()),
    }
}

pub(super) fn sanitize_gateway_failure(name: &str, detail: &str) -> Response {
    node_log::event(
        "warn",
        "live_app_gateway_error",
        &serde_json::json!({ "app": name, "error": detail }),
    );
    error_response(StatusCode::BAD_GATEWAY, "Upstream live app is unavailable.")
}

pub(super) fn validated_upstream_suffix<'a>(raw_path: &'a str, name: &str) -> Option<&'a str> {
    let base = format!("/apps/{name}");
    let suffix = raw_path.strip_prefix(&base)?;
    if !suffix.starts_with('/') || suffix.contains("//") || suffix.contains('\\') {
        return None;
    }
    if suffix.bytes().any(|b| b < 0x20 || b == 0x7f) {
        return None;
    }
    if suffix[1..].split('/').any(|seg| seg == "." || seg == "..") {
        return None;
    }
    if has_disallowed_percent_escape(suffix) {
        return None;
    }
    Some(suffix)
}

fn has_disallowed_percent_escape(input: &str) -> bool {
    let bytes = input.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' {
            let (Some(hi), Some(lo)) = (
                bytes.get(i + 1).copied().and_then(hex_nibble),
                bytes.get(i + 2).copied().and_then(hex_nibble),
            ) else {
                return true;
            };
            let decoded = (hi << 4) | lo;
            if decoded < 0x20
                || decoded == 0x7f
                || matches!(decoded, b'.' | b'/' | b'\\' | b'?' | b'#')
            {
                return true;
            }
            i += 3;
        } else {
            i += 1;
        }
    }
    false
}

fn hex_nibble(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

pub(super) fn connection_nominated_headers(headers: &HeaderMap) -> HashSet<String> {
    let mut nominated = HashSet::new();
    for value in headers.get_all(header::CONNECTION) {
        if let Ok(raw) = value.to_str() {
            for token in raw.split(',') {
                let name = token.trim().to_ascii_lowercase();
                if !name.is_empty() {
                    nominated.insert(name);
                }
            }
        }
    }
    nominated
}

pub(super) fn filter_cookie_header(raw: &str) -> Option<String> {
    let mut kept = Vec::new();
    for pair in raw.split(';') {
        let trimmed = pair.trim();
        if trimmed.is_empty() {
            continue;
        }
        let cookie_name = trimmed
            .split_once('=')
            .map_or(trimmed, |(k, _)| k.trim())
            .to_ascii_lowercase();
        if cookie_name.starts_with(RESERVED_COOKIE_PREFIX) {
            continue;
        }
        kept.push(trimmed);
    }
    if kept.is_empty() {
        None
    } else {
        Some(kept.join("; "))
    }
}

pub(super) fn has_disallowed_set_cookie(headers: &HeaderMap) -> bool {
    headers.get_all(header::SET_COOKIE).iter().any(|value| {
        String::from_utf8_lossy(value.as_bytes())
            .trim_start()
            .to_ascii_lowercase()
            .starts_with(RESERVED_COOKIE_PREFIX)
    })
}

pub(super) fn apply_request_headers(
    mut outgoing: reqwest::RequestBuilder,
    name: &str,
    headers: &HeaderMap,
) -> reqwest::RequestBuilder {
    let nominated = connection_nominated_headers(headers);
    let incoming_host = headers.get(header::HOST);
    for (key, value) in headers {
        let lower = key.as_str();
        if is_hop_by_hop_or_forwarded(lower)
            || nominated.contains(lower)
            || matches!(lower, "host" | "content-length")
        {
            continue;
        }
        if key == header::COOKIE {
            if let Ok(raw) = value.to_str()
                && let Some(filtered) = filter_cookie_header(raw)
            {
                outgoing = outgoing.header(header::COOKIE, filtered);
            }
            continue;
        }
        outgoing = outgoing.header(key.as_str(), value.as_bytes());
    }
    outgoing = outgoing.header(X_FORWARDED_PREFIX.as_str(), format!("/apps/{name}"));
    if let Some(host) = incoming_host {
        outgoing = outgoing.header(X_FORWARDED_HOST.as_str(), host.as_bytes());
    }
    let proto = forwarded_proto(headers);
    outgoing.header(X_FORWARDED_PROTO.as_str(), proto)
}

pub(super) fn forwarded_proto(headers: &HeaderMap) -> &'static str {
    headers
        .get(X_FORWARDED_PROTO)
        .and_then(|v| v.to_str().ok())
        .filter(|v| v.eq_ignore_ascii_case("https"))
        .map_or("http", |_| "https")
}

pub(super) fn rewrite_location(
    value: &HeaderValue,
    prefix: &str,
    port: u16,
) -> Option<HeaderValue> {
    let raw = value.to_str().ok()?;
    for origin in [
        format!("http://127.0.0.1:{port}"),
        format!("http://localhost:{port}"),
        format!("http://[::1]:{port}"),
    ] {
        if let Some(rest) = raw.strip_prefix(&origin)
            && (rest.is_empty() || rest.starts_with(['/', '?', '#']))
        {
            let rewritten = if rest.is_empty() {
                format!("{prefix}/")
            } else if already_prefixed(rest, prefix) {
                rest.to_owned()
            } else if rest.starts_with('/') {
                format!("{prefix}{rest}")
            } else {
                format!("{prefix}/{rest}")
            };
            return HeaderValue::from_str(&rewritten).ok();
        }
    }

    if !raw.starts_with('/') || raw.starts_with("//") || already_prefixed(raw, prefix) {
        return None;
    }
    HeaderValue::from_str(&format!("{prefix}{raw}")).ok()
}

fn already_prefixed(path: &str, prefix: &str) -> bool {
    path == prefix
        || path.starts_with(&format!("{prefix}/"))
        || path.starts_with(&format!("{prefix}?"))
        || path.starts_with(&format!("{prefix}#"))
}

pub(super) fn is_hop_by_hop_or_forwarded(name: &str) -> bool {
    is_hop_by_hop(name)
        || matches!(
            name,
            "x-forwarded-prefix" | "x-forwarded-host" | "x-forwarded-for" | "x-forwarded-proto"
        )
}

pub(super) fn is_hop_by_hop(name: &str) -> bool {
    matches!(
        name,
        "connection"
            | "keep-alive"
            | "proxy-authenticate"
            | "proxy-authorization"
            | "te"
            | "trailer"
            | "transfer-encoding"
            | "upgrade"
    )
}
