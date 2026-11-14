//! Streaming HTTP reverse proxy for `/apps/<name>/` backends on `127.0.0.1`.

use std::{collections::HashSet, time::Duration};

use axum::{
    body::Body,
    http::{HeaderMap, HeaderName, HeaderValue, Request, StatusCode, header},
    response::{IntoResponse, Response},
};
use renoa_local::{AppError, Apps};

const MAX_REQUEST_BODY_BYTES: usize = 64 * 1024 * 1024;
const UPSTREAM_CONNECT_TIMEOUT: Duration = Duration::from_secs(5);
const UPSTREAM_REQUEST_TIMEOUT: Duration = Duration::from_secs(60);
const X_FORWARDED_PREFIX: HeaderName = HeaderName::from_static("x-forwarded-prefix");
const X_FORWARDED_HOST: HeaderName = HeaderName::from_static("x-forwarded-host");

#[derive(Clone)]
pub(super) struct ProxyClient {
    client: reqwest::Client,
}

impl ProxyClient {
    pub(super) fn new() -> Result<Self, reqwest::Error> {
        let client = reqwest::Client::builder()
            .connect_timeout(UPSTREAM_CONNECT_TIMEOUT)
            .timeout(UPSTREAM_REQUEST_TIMEOUT)
            .redirect(reqwest::redirect::Policy::none())
            .no_proxy()
            .build()?;
        Ok(Self { client })
    }
}

pub(super) fn error_response(status: StatusCode, body: impl Into<String>) -> Response {
    (
        status,
        [(header::X_CONTENT_TYPE_OPTIONS, "nosniff")],
        body.into(),
    )
        .into_response()
}

pub(super) async fn forward(
    proxy: &ProxyClient,
    apps: &Apps,
    name: &str,
    request: Request<Body>,
) -> Response {
    let Some(upstream_suffix) = validated_upstream_suffix(request.uri().path(), name) else {
        return error_response(
            StatusCode::NOT_FOUND,
            "No live app is registered at this address.",
        );
    };
    let upstream_suffix = upstream_suffix.to_owned();

    let app = match apps.ensure_running(name).await {
        Ok(app) => app,
        Err(err) => return app_error_response(name, &err),
    };

    let outgoing =
        match build_upstream_request(proxy, name, app.port, &upstream_suffix, request).await {
            Ok(req) => req,
            Err(resp) => return *resp,
        };

    let upstream = match outgoing.send().await {
        Ok(resp) => resp,
        Err(err) => {
            return error_response(
                StatusCode::BAD_GATEWAY,
                format!(
                    "Upstream live app `{name}` did not answer on 127.0.0.1:{}: {err}",
                    app.port
                ),
            );
        }
    };

    let _ = apps.touch(name);
    build_downstream_response(name, upstream)
}

fn app_error_response(name: &str, err: &AppError) -> Response {
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
            let body = if logs.is_empty() {
                format!("Live app `{app_name}` failed: {reason}")
            } else {
                format!("Live app `{app_name}` failed: {reason}\n\nRecent logs:\n{logs}")
            };
            error_response(StatusCode::BAD_GATEWAY, body)
        }
        _ => error_response(
            StatusCode::BAD_GATEWAY,
            format!("Live app `{name}` could not start: {err}"),
        ),
    }
}

async fn build_upstream_request(
    proxy: &ProxyClient,
    name: &str,
    port: u16,
    upstream_suffix: &str,
    request: Request<Body>,
) -> Result<reqwest::RequestBuilder, Box<Response>> {
    let (parts, body) = request.into_parts();
    let Ok(method) = reqwest::Method::from_bytes(parts.method.as_str().as_bytes()) else {
        return Err(Box::new(error_response(
            StatusCode::BAD_REQUEST,
            "Unsupported HTTP method.",
        )));
    };
    let query = parts
        .uri
        .query()
        .map(|q| format!("?{q}"))
        .unwrap_or_default();
    let target_url = format!("http://127.0.0.1:{port}{upstream_suffix}{query}");

    let mut outgoing = apply_request_headers(
        proxy.client.request(method, &target_url),
        name,
        &parts.headers,
    );

    let Ok(body_bytes) = axum::body::to_bytes(body, MAX_REQUEST_BODY_BYTES).await else {
        return Err(Box::new(error_response(
            StatusCode::PAYLOAD_TOO_LARGE,
            "Request body exceeded 64 MiB gateway limit.",
        )));
    };
    if !body_bytes.is_empty() {
        outgoing = outgoing.body(body_bytes);
    }
    Ok(outgoing)
}

fn validated_upstream_suffix<'a>(raw_path: &'a str, name: &str) -> Option<&'a str> {
    let base = format!("/apps/{name}");
    let suffix = raw_path.strip_prefix(&base)?;
    if !suffix.starts_with('/') {
        return None;
    }
    if suffix.contains("//") || suffix.contains('\\') {
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

fn connection_nominated_headers(headers: &HeaderMap) -> HashSet<String> {
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

fn apply_request_headers(
    mut outgoing: reqwest::RequestBuilder,
    name: &str,
    headers: &HeaderMap,
) -> reqwest::RequestBuilder {
    let nominated = connection_nominated_headers(headers);
    let incoming_host = headers.get(header::HOST);
    for (key, value) in headers {
        let lower = key.as_str();
        if is_hop_by_hop(lower)
            || nominated.contains(lower)
            || matches!(
                lower,
                "host"
                    | "content-length"
                    | "x-forwarded-prefix"
                    | "x-forwarded-host"
                    | "x-forwarded-for"
                    | "x-forwarded-proto"
            )
        {
            continue;
        }
        outgoing = outgoing.header(key.as_str(), value.as_bytes());
    }
    outgoing = outgoing.header(X_FORWARDED_PREFIX.as_str(), format!("/apps/{name}"));
    if let Some(host) = incoming_host {
        outgoing = outgoing.header(X_FORWARDED_HOST.as_str(), host.as_bytes());
    }
    outgoing
}

fn build_downstream_response(name: &str, upstream: reqwest::Response) -> Response {
    let status =
        StatusCode::from_u16(upstream.status().as_u16()).unwrap_or(StatusCode::BAD_GATEWAY);
    let mut builder = Response::builder().status(status);
    let prefix = format!("/apps/{name}");
    let nominated = connection_nominated_headers(upstream.headers());
    for (key, value) in upstream.headers() {
        let lower = key.as_str();
        if is_hop_by_hop(lower) || nominated.contains(lower) {
            continue;
        }
        if key == header::LOCATION
            && let Some(rewritten) = rewrite_location(value, &prefix)
        {
            builder = builder.header(key, rewritten);
            continue;
        }
        builder = builder.header(key, value);
    }
    let stream = upstream.bytes_stream();
    builder
        .body(Body::from_stream(stream))
        .unwrap_or_else(|_| StatusCode::BAD_GATEWAY.into_response())
}

fn rewrite_location(value: &HeaderValue, prefix: &str) -> Option<HeaderValue> {
    let raw = value.to_str().ok()?;
    if !raw.starts_with('/') || raw.starts_with("//") {
        return None;
    }
    if raw == prefix
        || raw.starts_with(&format!("{prefix}/"))
        || raw.starts_with(&format!("{prefix}?"))
        || raw.starts_with(&format!("{prefix}#"))
    {
        return None;
    }
    HeaderValue::from_str(&format!("{prefix}{raw}")).ok()
}

fn is_hop_by_hop(name: &str) -> bool {
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
