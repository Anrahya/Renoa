//! Streaming HTTP reverse proxy for `/apps/<name>/` backends on `127.0.0.1`.

use std::{
    io,
    pin::Pin,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    task::{Context, Poll},
    time::Duration,
};

use axum::{
    body::{Body, Bytes},
    http::{HeaderMap, Request, StatusCode, header, request::Parts},
    response::{IntoResponse, Response},
};
use futures_util::{Stream, StreamExt as _, stream};
use renoa_local::{AppStreamGuard, Apps};
use tokio::sync::{OwnedSemaphorePermit, Semaphore};

use super::{
    http_policy::{
        app_error_response, apply_request_headers, connection_nominated_headers, error_response,
        has_disallowed_set_cookie, is_hop_by_hop, rewrite_location, sanitize_gateway_failure,
        validated_upstream_suffix,
    },
    ws,
};
use crate::node_log;

const MAX_REQUEST_BODY_BYTES: usize = 64 * 1024 * 1024;
const REPLAY_BUFFER_BYTES: usize = 64 * 1024;
const MAX_CONCURRENT_REQUESTS: usize = 64;
const UPSTREAM_CONNECT_TIMEOUT: Duration = Duration::from_secs(5);
const UPSTREAM_HEADER_TIMEOUT: Duration = Duration::from_secs(30);

#[derive(Clone)]
pub(super) struct ProxyClient {
    client: reqwest::Client,
    permits: Arc<Semaphore>,
}

impl ProxyClient {
    pub(super) fn new() -> Result<Self, reqwest::Error> {
        let client = reqwest::Client::builder()
            .connect_timeout(UPSTREAM_CONNECT_TIMEOUT)
            .redirect(reqwest::redirect::Policy::none())
            .no_proxy()
            .build()?;
        Ok(Self {
            client,
            permits: Arc::new(Semaphore::new(MAX_CONCURRENT_REQUESTS)),
        })
    }

    pub(super) async fn acquire_permit(&self) -> Option<OwnedSemaphorePermit> {
        Arc::clone(&self.permits).acquire_owned().await.ok()
    }
}

pub(super) async fn forward(
    proxy: &ProxyClient,
    apps: &Apps,
    name: &str,
    request: Request<Body>,
) -> Response {
    if ws::is_websocket_upgrade(request.headers()) {
        return ws::forward_websocket(proxy, apps, name, request).await;
    }

    let Some(_permit) = proxy.acquire_permit().await else {
        return error_response(
            StatusCode::SERVICE_UNAVAILABLE,
            "Site gateway concurrency limit reached.",
        );
    };
    let Some(upstream_suffix) = validated_upstream_suffix(request.uri().path(), name) else {
        return error_response(
            StatusCode::NOT_FOUND,
            "No live app is registered at this address.",
        );
    };
    let upstream_suffix = upstream_suffix.to_owned();
    let (parts, body) = request.into_parts();

    if let Some(rejection) = check_content_length(&parts.headers) {
        return *rejection;
    }

    let prepared = match read_replay_prefix(body).await {
        Ok(p) => p,
        Err(rejection) => return *rejection,
    };

    let (mut port, mut fast_path) = match apps.running_port(name) {
        Ok(Some(port)) => (port, true),
        Ok(None) => match apps.ensure_running(name).await {
            Ok(app) => (app.port, false),
            Err(err) => return app_error_response(name, &err),
        },
        Err(err) => return app_error_response(name, &err),
    };

    if fast_path && matches!(prepared, PreparedBody::Stream { .. }) {
        let app = match apps.ensure_running(name).await {
            Ok(app) => app,
            Err(err) => return app_error_response(name, &err),
        };
        port = app.port;
        fast_path = false;
    }

    match prepared {
        PreparedBody::Buffered(bytes) => {
            forward_buffered(
                proxy,
                apps,
                name,
                port,
                fast_path,
                &upstream_suffix,
                &parts,
                bytes,
            )
            .await
        }
        PreparedBody::Stream { prefix, remaining } => {
            forward_stream(
                proxy,
                apps,
                name,
                port,
                &upstream_suffix,
                &parts,
                prefix,
                remaining,
            )
            .await
        }
    }
}

enum PreparedBody {
    Buffered(Bytes),
    Stream {
        prefix: Bytes,
        remaining: Pin<Box<dyn Stream<Item = Result<Bytes, axum::Error>> + Send>>,
    },
}

fn check_content_length(headers: &HeaderMap) -> Option<Box<Response>> {
    let val = headers.get(header::CONTENT_LENGTH)?;
    let Ok(text) = val.to_str() else {
        return Some(Box::new(error_response(
            StatusCode::BAD_REQUEST,
            "Invalid Content-Length header.",
        )));
    };
    let Ok(len) = text.trim().parse::<u64>() else {
        return Some(Box::new(error_response(
            StatusCode::BAD_REQUEST,
            "Invalid Content-Length header.",
        )));
    };
    if len > MAX_REQUEST_BODY_BYTES as u64 {
        return Some(Box::new(error_response(
            StatusCode::PAYLOAD_TOO_LARGE,
            "Request body exceeded 64 MiB gateway limit.",
        )));
    }
    None
}

async fn read_replay_prefix(body: Body) -> Result<PreparedBody, Box<Response>> {
    let mut stream = Box::pin(body.into_data_stream());
    let mut buf = Vec::new();
    while let Some(chunk_res) = stream.next().await {
        let chunk = chunk_res.map_err(|_| {
            Box::new(error_response(
                StatusCode::BAD_REQUEST,
                "Malformed request body stream.",
            ))
        })?;
        if chunk.is_empty() {
            continue;
        }
        if buf.len().saturating_add(chunk.len()) > MAX_REQUEST_BODY_BYTES {
            return Err(Box::new(error_response(
                StatusCode::PAYLOAD_TOO_LARGE,
                "Request body exceeded 64 MiB gateway limit.",
            )));
        }
        buf.extend_from_slice(&chunk);
        if buf.len() > REPLAY_BUFFER_BYTES {
            return Ok(PreparedBody::Stream {
                prefix: Bytes::from(buf),
                remaining: stream,
            });
        }
    }
    Ok(PreparedBody::Buffered(Bytes::from(buf)))
}

#[allow(clippy::too_many_arguments)]
async fn forward_buffered(
    proxy: &ProxyClient,
    apps: &Apps,
    name: &str,
    mut port: u16,
    fast_path: bool,
    upstream_suffix: &str,
    parts: &Parts,
    bytes: Bytes,
) -> Response {
    for attempt in 0..2 {
        let mut outgoing = match build_base_request(proxy, name, port, upstream_suffix, parts) {
            Ok(req) => req,
            Err(rejection) => return *rejection,
        };
        if !bytes.is_empty() {
            outgoing = outgoing.body(bytes.clone());
        }
        match tokio::time::timeout(UPSTREAM_HEADER_TIMEOUT, outgoing.send()).await {
            Ok(Ok(upstream)) => {
                let _ = apps.touch(name);
                return build_downstream_response(apps, name, port, upstream);
            }
            Ok(Err(err)) if attempt == 0 && fast_path && err.is_connect() => {
                match apps.ensure_running(name).await {
                    Ok(app) => port = app.port,
                    Err(wake_err) => return app_error_response(name, &wake_err),
                }
            }
            Ok(Err(err)) => {
                return sanitize_gateway_failure(name, &format!("upstream send failed: {err}"));
            }
            Err(_) => {
                return sanitize_gateway_failure(
                    name,
                    "upstream timed out waiting for response headers",
                );
            }
        }
    }
    sanitize_gateway_failure(name, "upstream retries exhausted")
}

#[allow(clippy::too_many_arguments)]
async fn forward_stream(
    proxy: &ProxyClient,
    apps: &Apps,
    name: &str,
    port: u16,
    upstream_suffix: &str,
    parts: &Parts,
    prefix: Bytes,
    remaining: Pin<Box<dyn Stream<Item = Result<Bytes, axum::Error>> + Send>>,
) -> Response {
    let mut outgoing = match build_base_request(proxy, name, port, upstream_suffix, parts) {
        Ok(req) => req,
        Err(rejection) => return *rejection,
    };
    let initial_len = prefix.len();
    let exceeded = Arc::new(AtomicBool::new(false));
    let exceeded_flag = Arc::clone(&exceeded);
    let mut seen_bytes = initial_len;

    let head = stream::once(async move { Ok::<Bytes, io::Error>(prefix) });
    let tail = remaining.map(move |item| match item {
        Ok(chunk) => {
            seen_bytes = seen_bytes.saturating_add(chunk.len());
            if seen_bytes > MAX_REQUEST_BODY_BYTES {
                exceeded_flag.store(true, Ordering::Relaxed);
                Err(io::Error::other(
                    "request body exceeded 64 MiB gateway limit",
                ))
            } else {
                Ok(chunk)
            }
        }
        Err(err) => Err(io::Error::other(err.to_string())),
    });
    outgoing = outgoing.body(reqwest::Body::wrap_stream(head.chain(tail)));

    match tokio::time::timeout(UPSTREAM_HEADER_TIMEOUT, outgoing.send()).await {
        Ok(Ok(upstream)) => {
            let _ = apps.touch(name);
            build_downstream_response(apps, name, port, upstream)
        }
        Ok(Err(err)) => {
            if exceeded.load(Ordering::Relaxed) {
                error_response(
                    StatusCode::PAYLOAD_TOO_LARGE,
                    "Request body exceeded 64 MiB gateway limit.",
                )
            } else {
                sanitize_gateway_failure(name, &format!("upstream stream failed: {err}"))
            }
        }
        Err(_) => sanitize_gateway_failure(name, "upstream timed out waiting for response headers"),
    }
}

fn build_base_request(
    proxy: &ProxyClient,
    name: &str,
    port: u16,
    upstream_suffix: &str,
    parts: &Parts,
) -> Result<reqwest::RequestBuilder, Box<Response>> {
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
    Ok(apply_request_headers(
        proxy.client.request(method, &target_url),
        name,
        &parts.headers,
    ))
}

fn build_downstream_response(
    apps: &Apps,
    name: &str,
    port: u16,
    upstream: reqwest::Response,
) -> Response {
    if has_disallowed_set_cookie(upstream.headers()) {
        node_log::event(
            "warn",
            "live_app_disallowed_set_cookie",
            &serde_json::json!({ "app": name, "transport": "http" }),
        );
        return error_response(
            StatusCode::BAD_GATEWAY,
            "Upstream live app attempted to set a reserved cookie.",
        );
    }

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
            && let Some(rewritten) = rewrite_location(value, &prefix, port)
        {
            builder = builder.header(key, rewritten);
            continue;
        }
        builder = builder.header(key, value);
    }
    let guard = apps.acquire_stream(name);
    let guarded = GuardedByteStream {
        inner: Box::pin(upstream.bytes_stream()),
        _guard: guard,
    };
    builder
        .body(Body::from_stream(guarded))
        .unwrap_or_else(|_| StatusCode::BAD_GATEWAY.into_response())
}

struct GuardedByteStream<S> {
    inner: Pin<Box<S>>,
    _guard: AppStreamGuard,
}

impl<S, T, E> Stream for GuardedByteStream<S>
where
    S: Stream<Item = Result<T, E>>,
{
    type Item = Result<T, E>;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        self.inner.as_mut().poll_next(cx)
    }
}
