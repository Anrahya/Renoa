//! WebSocket reverse-proxy upgrade handling for `/apps/<name>/`.

use std::time::Duration;

use tokio::time::Instant;

use axum::{
    body::Body,
    extract::{
        FromRequestParts as _,
        ws::{CloseFrame, Message as AxumMessage, WebSocket, WebSocketUpgrade},
    },
    http::{HeaderMap, Request, StatusCode, header},
    response::Response,
};
use futures_util::{SinkExt as _, StreamExt as _};
use renoa_local::Apps;
use tokio_tungstenite::{
    connect_async,
    tungstenite::{
        Message as WsMessage,
        client::IntoClientRequest as _,
        protocol::{CloseFrame as WsCloseFrame, frame::coding::CloseCode},
    },
};

use super::{
    http_policy::{
        app_error_response, error_response, filter_cookie_header, forwarded_proto,
        has_disallowed_set_cookie, is_hop_by_hop_or_forwarded, sanitize_gateway_failure,
        validated_upstream_suffix,
    },
    proxy::ProxyClient,
};
use crate::node_log;

const WS_CONNECT_TIMEOUT: Duration = Duration::from_secs(15);

pub(super) fn is_websocket_upgrade(headers: &HeaderMap) -> bool {
    let upgrade_ws = headers
        .get(header::UPGRADE)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|v| v.trim().eq_ignore_ascii_case("websocket"));
    if !upgrade_ws {
        return false;
    }
    headers.get_all(header::CONNECTION).iter().any(|value| {
        value.to_str().is_ok_and(|raw| {
            raw.split(',')
                .any(|token| token.trim().eq_ignore_ascii_case("upgrade"))
        })
    })
}

pub(super) async fn forward_websocket(
    proxy: &ProxyClient,
    apps: &Apps,
    name: &str,
    request: Request<Body>,
) -> Response {
    let Some(permit) = proxy.try_websocket_permit() else {
        return error_response(
            StatusCode::SERVICE_UNAVAILABLE,
            "Site gateway WebSocket limit reached.",
        );
    };
    let Some(upstream_suffix) = validated_upstream_suffix(request.uri().path(), name) else {
        return error_response(
            StatusCode::NOT_FOUND,
            "No live app is registered at this address.",
        );
    };
    let upstream_suffix = upstream_suffix.to_owned();
    let (mut parts, _body) = request.into_parts();
    let ws_upgrade = match WebSocketUpgrade::from_request_parts(&mut parts, &()).await {
        Ok(upgrade) => upgrade,
        Err(rejection) => {
            node_log::event(
                "warn",
                "live_app_ws_upgrade_rejected",
                &serde_json::json!({ "app": name, "reason": rejection.to_string() }),
            );
            return error_response(
                StatusCode::BAD_REQUEST,
                "Invalid WebSocket upgrade request.",
            );
        }
    };

    let query = parts
        .uri
        .query()
        .map(|q| format!("?{q}"))
        .unwrap_or_default();

    let (port, fast_path) = match apps.running_port(name) {
        Ok(Some(port)) => (port, true),
        Ok(None) => match apps.ensure_running(name).await {
            Ok(app) => (app.port, false),
            Err(err) => return app_error_response(name, &err),
        },
        Err(err) => return app_error_response(name, &err),
    };

    let mut connected =
        connect_upstream_ws(port, &upstream_suffix, &query, name, &parts.headers).await;
    if connected.is_err() && fast_path {
        let app = match apps.ensure_running(name).await {
            Ok(app) => app,
            Err(err) => return app_error_response(name, &err),
        };
        connected =
            connect_upstream_ws(app.port, &upstream_suffix, &query, name, &parts.headers).await;
    }

    let (upstream_ws, upstream_resp) = match connected {
        Ok(ok) => ok,
        Err(err) => {
            return sanitize_gateway_failure(name, &format!("websocket connect failed: {err}"));
        }
    };

    if has_disallowed_set_cookie(upstream_resp.headers()) {
        node_log::event(
            "warn",
            "live_app_disallowed_set_cookie",
            &serde_json::json!({ "app": name, "transport": "websocket" }),
        );
        return error_response(
            StatusCode::BAD_GATEWAY,
            "Upstream live app attempted to set a reserved cookie.",
        );
    }

    let upgrade = if let Some(protocol) = upstream_resp
        .headers()
        .get(header::SEC_WEBSOCKET_PROTOCOL)
        .and_then(|v| v.to_str().ok())
    {
        ws_upgrade.protocols([protocol.to_owned()])
    } else {
        ws_upgrade
    };

    let app_name = name.to_owned();
    let apps_clone = apps.clone();
    let _ = apps.touch(name);
    upgrade.on_upgrade(move |client_ws| async move {
        let _permit = permit;
        let _stream_guard = apps_clone.acquire_stream(&app_name);
        relay_websocket(client_ws, upstream_ws, apps_clone.idle_timeout()).await;
    })
}

async fn connect_upstream_ws(
    port: u16,
    upstream_suffix: &str,
    query: &str,
    name: &str,
    incoming_headers: &HeaderMap,
) -> Result<
    (
        tokio_tungstenite::WebSocketStream<
            tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
        >,
        axum::http::Response<Option<Vec<u8>>>,
    ),
    String,
> {
    let url = format!("ws://127.0.0.1:{port}{upstream_suffix}{query}");
    let mut ws_req = url
        .into_client_request()
        .map_err(|err| format!("build websocket request: {err}"))?;
    let out_headers = ws_req.headers_mut();
    for (key, value) in incoming_headers {
        let lower = key.as_str();
        if is_hop_by_hop_or_forwarded(lower)
            || matches!(
                lower,
                "host"
                    | "content-length"
                    | "sec-websocket-key"
                    | "sec-websocket-version"
                    | "sec-websocket-extensions"
            )
        {
            continue;
        }
        if key == header::COOKIE {
            if let Ok(raw) = value.to_str()
                && let Some(filtered) = filter_cookie_header(raw)
                && let Ok(hv) = filtered.parse()
            {
                out_headers.append(header::COOKIE, hv);
            }
            continue;
        }
        out_headers.append(key.clone(), value.clone());
    }
    if let Ok(hv) = format!("/apps/{name}").parse() {
        out_headers.insert("x-forwarded-prefix", hv);
    }
    if let Some(host) = incoming_headers.get(header::HOST) {
        out_headers.insert("x-forwarded-host", host.clone());
    }
    let proto = forwarded_proto(incoming_headers);
    if let Ok(hv) = proto.parse() {
        out_headers.insert("x-forwarded-proto", hv);
    }

    tokio::time::timeout(WS_CONNECT_TIMEOUT, connect_async(ws_req))
        .await
        .map_err(|_| "websocket upstream connect timed out".to_owned())?
        .map_err(|err| err.to_string())
}

/// Relays both directions independently, so a peer that stops reading
/// blocks only its own direction, and closes after `idle_timeout` without a
/// message either way so an open socket cannot keep its app awake forever.
async fn relay_websocket(
    client_ws: WebSocket,
    upstream_ws: tokio_tungstenite::WebSocketStream<
        tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
    >,
    idle_timeout: Duration,
) {
    let (mut client_tx, mut client_rx) = client_ws.split();
    let (mut up_tx, mut up_rx) = upstream_ws.split();
    let last_active = std::sync::Mutex::new(Instant::now());
    let touch = || {
        *last_active
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Instant::now();
    };
    let to_upstream = async {
        while let Some(Ok(msg)) = client_rx.next().await {
            touch();
            let close = matches!(msg, AxumMessage::Close(_));
            if up_tx.send(axum_to_tungstenite(msg)).await.is_err() || close {
                break;
            }
        }
    };
    let to_client = async {
        while let Some(Ok(msg)) = up_rx.next().await {
            touch();
            let close = matches!(msg, WsMessage::Close(_));
            if let Some(mapped) = tungstenite_to_axum(msg)
                && client_tx.send(mapped).await.is_err()
            {
                break;
            }
            if close {
                break;
            }
        }
    };
    let idle = async {
        loop {
            let deadline = *last_active
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                + idle_timeout;
            if deadline <= Instant::now() {
                break;
            }
            tokio::time::sleep_until(deadline).await;
        }
    };
    tokio::select! {
        () = to_upstream => {}
        () = to_client => {}
        () = idle => {}
    }
}

fn axum_to_tungstenite(msg: AxumMessage) -> WsMessage {
    match msg {
        AxumMessage::Text(text) => WsMessage::Text(text.as_str().into()),
        AxumMessage::Binary(data) => WsMessage::Binary(data),
        AxumMessage::Ping(data) => WsMessage::Ping(data),
        AxumMessage::Pong(data) => WsMessage::Pong(data),
        AxumMessage::Close(frame) => WsMessage::Close(frame.map(|f| WsCloseFrame {
            code: CloseCode::from(f.code),
            reason: f.reason.as_str().into(),
        })),
    }
}

fn tungstenite_to_axum(msg: WsMessage) -> Option<AxumMessage> {
    match msg {
        WsMessage::Text(text) => Some(AxumMessage::Text(text.as_str().into())),
        WsMessage::Binary(data) => Some(AxumMessage::Binary(data)),
        WsMessage::Ping(data) => Some(AxumMessage::Ping(data)),
        WsMessage::Pong(data) => Some(AxumMessage::Pong(data)),
        WsMessage::Close(frame) => Some(AxumMessage::Close(frame.map(|f| CloseFrame {
            code: u16::from(f.code),
            reason: f.reason.as_str().into(),
        }))),
        WsMessage::Frame(_) => None,
    }
}
